use crate::{
    MoltObject, TYPE_ID_BYTEARRAY, TYPE_ID_BYTES, TYPE_ID_LIST, TYPE_ID_TUPLE, TYPE_ID_TYPE,
    alloc_bytearray, alloc_tuple, bytes_data, bytes_len, obj_from_bits, object_type_id,
    raise_exception, seq_vec_ref, string_obj_to_owned, to_f64, to_i64,
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
        raise_exception::<_>(_py, "ValueError", &format!("{role} must be non-negative"))
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

unsafe fn require_class_ptr(
    _py: &crate::PyToken<'_>,
    bits: u64,
    role: &str,
) -> Result<*mut u8, u64> {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{role} must be a class object"),
        ));
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_TYPE {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{role} must be a class object"),
        ));
    }
    Ok(ptr)
}

fn normalize_shape_bits(_py: &crate::PyToken<'_>, bits: u64) -> Result<(u64, bool), u64> {
    let obj = obj_from_bits(bits);
    if let Some(ptr) = obj.as_ptr() {
        return match unsafe { object_type_id(ptr) } {
            TYPE_ID_TUPLE => Ok((bits, false)),
            TYPE_ID_LIST => {
                let tuple_ptr = alloc_tuple(_py, unsafe { seq_vec_ref(ptr) });
                if tuple_ptr.is_null() {
                    Err(MoltObject::none().bits())
                } else {
                    Ok((MoltObject::from_ptr(tuple_ptr).bits(), true))
                }
            }
            _ => {
                if to_i64(obj).is_some() {
                    let tuple_ptr = alloc_tuple(_py, &[bits]);
                    if tuple_ptr.is_null() {
                        Err(MoltObject::none().bits())
                    } else {
                        Ok((MoltObject::from_ptr(tuple_ptr).bits(), true))
                    }
                } else {
                    Err(raise_exception::<_>(
                        _py,
                        "TypeError",
                        "shape must be a tuple, list, or int",
                    ))
                }
            }
        };
    }
    if to_i64(obj).is_some() {
        let tuple_ptr = alloc_tuple(_py, &[bits]);
        if tuple_ptr.is_null() {
            Err(MoltObject::none().bits())
        } else {
            Ok((MoltObject::from_ptr(tuple_ptr).bits(), true))
        }
    } else {
        Err(raise_exception::<_>(
            _py,
            "TypeError",
            "shape must be a tuple, list, or int",
        ))
    }
}

unsafe fn set_object_attr_bytes(
    _py: &crate::PyToken<'_>,
    obj_ptr: *mut u8,
    name: &[u8],
    name_str: &str,
    val_bits: u64,
) -> Result<(), u64> {
    let Some(name_bits) = crate::attr_name_bits_from_bytes(_py, name) else {
        return Err(MoltObject::none().bits());
    };
    let out = unsafe {
        crate::builtins::attributes::object_setattr_raw(_py, obj_ptr, name_bits, name_str, val_bits)
    } as u64;
    crate::dec_ref_bits(_py, name_bits);
    if crate::exception_pending(_py) {
        return Err(out);
    }
    Ok(())
}

unsafe fn object_attr_bits(
    _py: &crate::PyToken<'_>,
    obj_bits: u64,
    name: &[u8],
    name_str: &str,
) -> Result<u64, u64> {
    let Some(name_bits) = crate::attr_name_bits_from_bytes(_py, name) else {
        return Err(MoltObject::none().bits());
    };
    let out = crate::builtins::attributes::molt_get_attr_name(obj_bits, name_bits);
    crate::dec_ref_bits(_py, name_bits);
    if crate::exception_pending(_py) {
        return Err(out);
    }
    if obj_from_bits(out).is_none() {
        return Err(raise_exception::<_>(
            _py,
            "AttributeError",
            &format!("object has no attribute {:?}", name_str),
        ));
    }
    Ok(out)
}

unsafe fn build_buffer_instance(
    _py: &crate::PyToken<'_>,
    buffer_class_bits: u64,
    data_bits: u64,
    element_type_bits: u64,
    size: usize,
    format_bits: u64,
    itemsize: usize,
) -> Result<u64, u64> {
    let buffer_class_ptr = unsafe { require_class_ptr(_py, buffer_class_bits, "buffer_class")? };
    let buffer_bits = unsafe { crate::alloc_instance_for_class(_py, buffer_class_ptr) };
    let Some(buffer_ptr) = obj_from_bits(buffer_bits).as_ptr() else {
        return Err(buffer_bits);
    };
    let size_bits = MoltObject::from_int(size as i64).bits();
    let itemsize_bits = MoltObject::from_int(itemsize as i64).bits();
    if unsafe { set_object_attr_bytes(_py, buffer_ptr, b"_data", "_data", data_bits) }.is_err()
        || unsafe {
            set_object_attr_bytes(
                _py,
                buffer_ptr,
                b"_element_type",
                "_element_type",
                element_type_bits,
            )
        }
        .is_err()
        || unsafe { set_object_attr_bytes(_py, buffer_ptr, b"_size", "_size", size_bits) }.is_err()
        || unsafe {
            set_object_attr_bytes(
                _py,
                buffer_ptr,
                b"_format_char",
                "_format_char",
                format_bits,
            )
        }
        .is_err()
        || unsafe {
            set_object_attr_bytes(_py, buffer_ptr, b"_itemsize", "_itemsize", itemsize_bits)
        }
        .is_err()
    {
        crate::dec_ref_bits(_py, buffer_bits);
        return Err(MoltObject::none().bits());
    }
    Ok(buffer_bits)
}

unsafe fn build_tensor_instance(
    _py: &crate::PyToken<'_>,
    tensor_class_bits: u64,
    buf_bits: u64,
    shape_bits: u64,
    dtype_bits: u64,
) -> Result<u64, u64> {
    let tensor_class_ptr = unsafe { require_class_ptr(_py, tensor_class_bits, "tensor_class")? };
    let tensor_bits = unsafe { crate::alloc_instance_for_class(_py, tensor_class_ptr) };
    let Some(tensor_ptr) = obj_from_bits(tensor_bits).as_ptr() else {
        return Err(tensor_bits);
    };
    if unsafe { set_object_attr_bytes(_py, tensor_ptr, b"_buf", "_buf", buf_bits) }.is_err()
        || unsafe { set_object_attr_bytes(_py, tensor_ptr, b"_shape", "_shape", shape_bits) }
            .is_err()
        || unsafe { set_object_attr_bytes(_py, tensor_ptr, b"_dtype", "_dtype", dtype_bits) }
            .is_err()
    {
        crate::dec_ref_bits(_py, tensor_bits);
        return Err(MoltObject::none().bits());
    }
    Ok(tensor_bits)
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

#[inline]
unsafe fn aligned_f32_slice<'a>(ptr: *const u8, len: usize) -> Option<&'a [f32]> {
    if !(ptr as usize).is_multiple_of(std::mem::align_of::<f32>()) {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(ptr as *const f32, len) })
}

#[inline]
unsafe fn aligned_f32_slice_mut<'a>(ptr: *mut u8, len: usize) -> Option<&'a mut [f32]> {
    if !(ptr as usize).is_multiple_of(std::mem::align_of::<f32>()) {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts_mut(ptr as *mut f32, len) })
}

#[cfg(target_arch = "aarch64")]
#[inline]
unsafe fn load_f32x4_bytes_unaligned(ptr: *const u8) -> core::arch::aarch64::float32x4_t {
    use core::arch::{aarch64::float32x4_t, asm};
    let out: float32x4_t;
    unsafe {
        asm!(
            "ldr {out:q}, [{ptr}]",
            ptr = in(reg) ptr,
            out = lateout(vreg) out,
            options(readonly, nostack, preserves_flags),
        );
    }
    out
}

#[cfg(target_arch = "aarch64")]
#[inline]
unsafe fn load_f32_bytes_unaligned(ptr: *const u8) -> f32 {
    use core::arch::asm;
    let out: f32;
    unsafe {
        asm!(
            "ldr {out:s}, [{ptr}]",
            ptr = in(reg) ptr,
            out = lateout(vreg) out,
            options(readonly, nostack, preserves_flags),
        );
    }
    out
}

#[cfg(target_arch = "aarch64")]
#[inline]
unsafe fn store_f32_bytes_unaligned(ptr: *mut u8, value: f32) {
    use core::arch::asm;
    unsafe {
        asm!(
            "str {value:s}, [{ptr}]",
            ptr = in(reg) ptr,
            value = in(vreg) value,
            options(nostack, preserves_flags),
        );
    }
}

#[cfg(target_arch = "aarch64")]
#[inline]
unsafe fn linear_dot_ptrs_unaligned(
    x_row_ptr: *const u8,
    w_row_ptr: *const u8,
    in_features: usize,
) -> f32 {
    use core::arch::aarch64::{vaddvq_f32, vdupq_n_f32, vfmaq_f32};
    let mut acc = unsafe { vdupq_n_f32(0.0) };
    let mut x_cur = x_row_ptr;
    let mut w_cur = w_row_ptr;
    let mut remaining = in_features;
    while remaining >= 4 {
        let x = unsafe { load_f32x4_bytes_unaligned(x_cur) };
        let w = unsafe { load_f32x4_bytes_unaligned(w_cur) };
        acc = unsafe { vfmaq_f32(acc, x, w) };
        x_cur = unsafe { x_cur.add(16) };
        w_cur = unsafe { w_cur.add(16) };
        remaining -= 4;
    }
    let mut sum = unsafe { vaddvq_f32(acc) };
    while remaining > 0 {
        let x = unsafe { load_f32_bytes_unaligned(x_cur) };
        let w = unsafe { load_f32_bytes_unaligned(w_cur) };
        sum += x * w;
        x_cur = unsafe { x_cur.add(4) };
        w_cur = unsafe { w_cur.add(4) };
        remaining -= 1;
    }
    sum
}

#[cfg(target_arch = "aarch64")]
#[inline]
unsafe fn linear_dot4_unaligned(
    x_ptr: *const u8,
    x_off: usize,
    w_ptr: *const u8,
    w_off: usize,
    in_features: usize,
) -> f32 {
    let x_row_ptr = unsafe { x_ptr.add(x_off * 4) };
    let w_row_ptr = unsafe { w_ptr.add(w_off * 4) };
    unsafe { linear_dot_ptrs_unaligned(x_row_ptr, w_row_ptr, in_features) }
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn horizontal_sum_f32x4(acc: std::arch::x86_64::__m128) -> f32 {
    use std::arch::x86_64::*;
    let hi = unsafe { _mm_movehl_ps(acc, acc) };
    let sum2 = unsafe { _mm_add_ps(acc, hi) };
    let shuffled = unsafe { _mm_shuffle_ps(sum2, sum2, 0x55) };
    let sum1 = unsafe { _mm_add_ss(sum2, shuffled) };
    unsafe { _mm_cvtss_f32(sum1) }
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn linear_dot_ptrs_unaligned(
    x_row_ptr: *const u8,
    w_row_ptr: *const u8,
    in_features: usize,
) -> f32 {
    use std::arch::x86_64::*;
    let mut acc = unsafe { _mm_setzero_ps() };
    let mut x_cur = x_row_ptr;
    let mut w_cur = w_row_ptr;
    let mut remaining = in_features;
    while remaining >= 4 {
        let x = unsafe { _mm_loadu_ps(x_cur as *const f32) };
        let w = unsafe { _mm_loadu_ps(w_cur as *const f32) };
        acc = unsafe { _mm_add_ps(acc, _mm_mul_ps(x, w)) };
        x_cur = unsafe { x_cur.add(16) };
        w_cur = unsafe { w_cur.add(16) };
        remaining -= 4;
    }
    let mut sum = unsafe { horizontal_sum_f32x4(acc) };
    while remaining > 0 {
        let x = unsafe { (x_cur as *const f32).read_unaligned() };
        let w = unsafe { (w_cur as *const f32).read_unaligned() };
        sum += x * w;
        x_cur = unsafe { x_cur.add(4) };
        w_cur = unsafe { w_cur.add(4) };
        remaining -= 1;
    }
    sum
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn linear_dot4_unaligned(
    x_ptr: *const u8,
    x_off: usize,
    w_ptr: *const u8,
    w_off: usize,
    in_features: usize,
) -> f32 {
    let x_row_ptr = unsafe { x_ptr.add(x_off * 4) };
    let w_row_ptr = unsafe { w_ptr.add(w_off * 4) };
    unsafe { linear_dot_ptrs_unaligned(x_row_ptr, w_row_ptr, in_features) }
}

#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
#[inline]
unsafe fn horizontal_sum_f32x4(acc: std::arch::wasm32::v128) -> f32 {
    use std::arch::wasm32::*;
    unsafe {
        f32x4_extract_lane::<0>(acc)
            + f32x4_extract_lane::<1>(acc)
            + f32x4_extract_lane::<2>(acc)
            + f32x4_extract_lane::<3>(acc)
    }
}

#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
#[inline]
unsafe fn linear_dot_ptrs_unaligned(
    x_row_ptr: *const u8,
    w_row_ptr: *const u8,
    in_features: usize,
) -> f32 {
    use std::arch::wasm32::*;
    let mut acc = unsafe { f32x4_splat(0.0) };
    let mut x_cur = x_row_ptr;
    let mut w_cur = w_row_ptr;
    let mut remaining = in_features;
    while remaining >= 4 {
        let x = unsafe { v128_load(x_cur as *const v128) };
        let w = unsafe { v128_load(w_cur as *const v128) };
        acc = unsafe { f32x4_add(acc, f32x4_mul(x, w)) };
        x_cur = unsafe { x_cur.add(16) };
        w_cur = unsafe { w_cur.add(16) };
        remaining -= 4;
    }
    let mut sum = unsafe { horizontal_sum_f32x4(acc) };
    while remaining > 0 {
        let x = unsafe { (x_cur as *const f32).read_unaligned() };
        let w = unsafe { (w_cur as *const f32).read_unaligned() };
        sum += x * w;
        x_cur = unsafe { x_cur.add(4) };
        w_cur = unsafe { w_cur.add(4) };
        remaining -= 1;
    }
    sum
}

#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
#[inline]
unsafe fn linear_dot4_unaligned(
    x_ptr: *const u8,
    x_off: usize,
    w_ptr: *const u8,
    w_off: usize,
    in_features: usize,
) -> f32 {
    let x_row_ptr = unsafe { x_ptr.add(x_off * 4) };
    let w_row_ptr = unsafe { w_ptr.add(w_off * 4) };
    unsafe { linear_dot_ptrs_unaligned(x_row_ptr, w_row_ptr, in_features) }
}

#[cfg(all(target_arch = "aarch64", test))]
#[inline]
unsafe fn linear_dot4_rows_ptrs_unaligned(
    x_row_ptr: *const u8,
    row_ptrs: [*const u8; 4],
    in_features: usize,
) -> [f32; 4] {
    use core::arch::aarch64::{vaddvq_f32, vdupq_n_f32, vfmaq_f32};
    let mut acc0 = unsafe { vdupq_n_f32(0.0) };
    let mut acc1 = unsafe { vdupq_n_f32(0.0) };
    let mut acc2 = unsafe { vdupq_n_f32(0.0) };
    let mut acc3 = unsafe { vdupq_n_f32(0.0) };
    let mut x_cur = x_row_ptr;
    let mut w0_cur = row_ptrs[0];
    let mut w1_cur = row_ptrs[1];
    let mut w2_cur = row_ptrs[2];
    let mut w3_cur = row_ptrs[3];
    let mut remaining = in_features;
    while remaining >= 4 {
        let x = unsafe { load_f32x4_bytes_unaligned(x_cur) };
        let w0 = unsafe { load_f32x4_bytes_unaligned(w0_cur) };
        let w1 = unsafe { load_f32x4_bytes_unaligned(w1_cur) };
        let w2 = unsafe { load_f32x4_bytes_unaligned(w2_cur) };
        let w3 = unsafe { load_f32x4_bytes_unaligned(w3_cur) };
        acc0 = unsafe { vfmaq_f32(acc0, x, w0) };
        acc1 = unsafe { vfmaq_f32(acc1, x, w1) };
        acc2 = unsafe { vfmaq_f32(acc2, x, w2) };
        acc3 = unsafe { vfmaq_f32(acc3, x, w3) };
        x_cur = unsafe { x_cur.add(16) };
        w0_cur = unsafe { w0_cur.add(16) };
        w1_cur = unsafe { w1_cur.add(16) };
        w2_cur = unsafe { w2_cur.add(16) };
        w3_cur = unsafe { w3_cur.add(16) };
        remaining -= 4;
    }
    let mut sum0 = unsafe { vaddvq_f32(acc0) };
    let mut sum1 = unsafe { vaddvq_f32(acc1) };
    let mut sum2 = unsafe { vaddvq_f32(acc2) };
    let mut sum3 = unsafe { vaddvq_f32(acc3) };
    while remaining > 0 {
        let x = unsafe { (x_cur as *const f32).read_unaligned() };
        let w0 = unsafe { (w0_cur as *const f32).read_unaligned() };
        let w1 = unsafe { (w1_cur as *const f32).read_unaligned() };
        let w2 = unsafe { (w2_cur as *const f32).read_unaligned() };
        let w3 = unsafe { (w3_cur as *const f32).read_unaligned() };
        sum0 += x * w0;
        sum1 += x * w1;
        sum2 += x * w2;
        sum3 += x * w3;
        x_cur = unsafe { x_cur.add(4) };
        w0_cur = unsafe { w0_cur.add(4) };
        w1_cur = unsafe { w1_cur.add(4) };
        w2_cur = unsafe { w2_cur.add(4) };
        w3_cur = unsafe { w3_cur.add(4) };
        remaining -= 1;
    }
    [sum0, sum1, sum2, sum3]
}

#[cfg(target_arch = "aarch64")]
#[inline]
unsafe fn linear_rows4_store_ptrs_unaligned(
    x_row_ptr: *const u8,
    row_ptrs: [*const u8; 4],
    out_ptrs: [*mut u8; 4],
    in_features: usize,
) {
    use core::arch::aarch64::{vaddvq_f32, vdupq_n_f32, vfmaq_f32};
    let mut acc0 = unsafe { vdupq_n_f32(0.0) };
    let mut acc1 = unsafe { vdupq_n_f32(0.0) };
    let mut acc2 = unsafe { vdupq_n_f32(0.0) };
    let mut acc3 = unsafe { vdupq_n_f32(0.0) };
    let mut x_cur = x_row_ptr;
    let mut w0_cur = row_ptrs[0];
    let mut w1_cur = row_ptrs[1];
    let mut w2_cur = row_ptrs[2];
    let mut w3_cur = row_ptrs[3];
    let mut remaining = in_features;
    while remaining >= 4 {
        let x = unsafe { load_f32x4_bytes_unaligned(x_cur) };
        let w0 = unsafe { load_f32x4_bytes_unaligned(w0_cur) };
        let w1 = unsafe { load_f32x4_bytes_unaligned(w1_cur) };
        let w2 = unsafe { load_f32x4_bytes_unaligned(w2_cur) };
        let w3 = unsafe { load_f32x4_bytes_unaligned(w3_cur) };
        acc0 = unsafe { vfmaq_f32(acc0, x, w0) };
        acc1 = unsafe { vfmaq_f32(acc1, x, w1) };
        acc2 = unsafe { vfmaq_f32(acc2, x, w2) };
        acc3 = unsafe { vfmaq_f32(acc3, x, w3) };
        x_cur = unsafe { x_cur.add(16) };
        w0_cur = unsafe { w0_cur.add(16) };
        w1_cur = unsafe { w1_cur.add(16) };
        w2_cur = unsafe { w2_cur.add(16) };
        w3_cur = unsafe { w3_cur.add(16) };
        remaining -= 4;
    }
    let mut sum0 = unsafe { vaddvq_f32(acc0) };
    let mut sum1 = unsafe { vaddvq_f32(acc1) };
    let mut sum2 = unsafe { vaddvq_f32(acc2) };
    let mut sum3 = unsafe { vaddvq_f32(acc3) };
    while remaining > 0 {
        let x = unsafe { load_f32_bytes_unaligned(x_cur) };
        let w0 = unsafe { load_f32_bytes_unaligned(w0_cur) };
        let w1 = unsafe { load_f32_bytes_unaligned(w1_cur) };
        let w2 = unsafe { load_f32_bytes_unaligned(w2_cur) };
        let w3 = unsafe { load_f32_bytes_unaligned(w3_cur) };
        sum0 += x * w0;
        sum1 += x * w1;
        sum2 += x * w2;
        sum3 += x * w3;
        x_cur = unsafe { x_cur.add(4) };
        w0_cur = unsafe { w0_cur.add(4) };
        w1_cur = unsafe { w1_cur.add(4) };
        w2_cur = unsafe { w2_cur.add(4) };
        w3_cur = unsafe { w3_cur.add(4) };
        remaining -= 1;
    }
    unsafe {
        store_f32_bytes_unaligned(out_ptrs[0], sum0);
        store_f32_bytes_unaligned(out_ptrs[1], sum1);
        store_f32_bytes_unaligned(out_ptrs[2], sum2);
        store_f32_bytes_unaligned(out_ptrs[3], sum3);
    }
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn linear_rows4_store_ptrs_unaligned(
    x_row_ptr: *const u8,
    row_ptrs: [*const u8; 4],
    out_ptrs: [*mut u8; 4],
    in_features: usize,
) {
    use std::arch::x86_64::*;
    let mut acc0 = unsafe { _mm_setzero_ps() };
    let mut acc1 = unsafe { _mm_setzero_ps() };
    let mut acc2 = unsafe { _mm_setzero_ps() };
    let mut acc3 = unsafe { _mm_setzero_ps() };
    let mut x_cur = x_row_ptr;
    let mut w0_cur = row_ptrs[0];
    let mut w1_cur = row_ptrs[1];
    let mut w2_cur = row_ptrs[2];
    let mut w3_cur = row_ptrs[3];
    let mut remaining = in_features;
    while remaining >= 4 {
        let x = unsafe { _mm_loadu_ps(x_cur as *const f32) };
        let w0 = unsafe { _mm_loadu_ps(w0_cur as *const f32) };
        let w1 = unsafe { _mm_loadu_ps(w1_cur as *const f32) };
        let w2 = unsafe { _mm_loadu_ps(w2_cur as *const f32) };
        let w3 = unsafe { _mm_loadu_ps(w3_cur as *const f32) };
        acc0 = unsafe { _mm_add_ps(acc0, _mm_mul_ps(x, w0)) };
        acc1 = unsafe { _mm_add_ps(acc1, _mm_mul_ps(x, w1)) };
        acc2 = unsafe { _mm_add_ps(acc2, _mm_mul_ps(x, w2)) };
        acc3 = unsafe { _mm_add_ps(acc3, _mm_mul_ps(x, w3)) };
        x_cur = unsafe { x_cur.add(16) };
        w0_cur = unsafe { w0_cur.add(16) };
        w1_cur = unsafe { w1_cur.add(16) };
        w2_cur = unsafe { w2_cur.add(16) };
        w3_cur = unsafe { w3_cur.add(16) };
        remaining -= 4;
    }
    let mut sum0 = unsafe { horizontal_sum_f32x4(acc0) };
    let mut sum1 = unsafe { horizontal_sum_f32x4(acc1) };
    let mut sum2 = unsafe { horizontal_sum_f32x4(acc2) };
    let mut sum3 = unsafe { horizontal_sum_f32x4(acc3) };
    while remaining > 0 {
        let x = unsafe { (x_cur as *const f32).read_unaligned() };
        let w0 = unsafe { (w0_cur as *const f32).read_unaligned() };
        let w1 = unsafe { (w1_cur as *const f32).read_unaligned() };
        let w2 = unsafe { (w2_cur as *const f32).read_unaligned() };
        let w3 = unsafe { (w3_cur as *const f32).read_unaligned() };
        sum0 += x * w0;
        sum1 += x * w1;
        sum2 += x * w2;
        sum3 += x * w3;
        x_cur = unsafe { x_cur.add(4) };
        w0_cur = unsafe { w0_cur.add(4) };
        w1_cur = unsafe { w1_cur.add(4) };
        w2_cur = unsafe { w2_cur.add(4) };
        w3_cur = unsafe { w3_cur.add(4) };
        remaining -= 1;
    }
    unsafe {
        (out_ptrs[0] as *mut f32).write_unaligned(sum0);
        (out_ptrs[1] as *mut f32).write_unaligned(sum1);
        (out_ptrs[2] as *mut f32).write_unaligned(sum2);
        (out_ptrs[3] as *mut f32).write_unaligned(sum3);
    }
}

#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
#[inline]
unsafe fn linear_rows4_store_ptrs_unaligned(
    x_row_ptr: *const u8,
    row_ptrs: [*const u8; 4],
    out_ptrs: [*mut u8; 4],
    in_features: usize,
) {
    use std::arch::wasm32::*;
    let mut acc0 = unsafe { f32x4_splat(0.0) };
    let mut acc1 = unsafe { f32x4_splat(0.0) };
    let mut acc2 = unsafe { f32x4_splat(0.0) };
    let mut acc3 = unsafe { f32x4_splat(0.0) };
    let mut x_cur = x_row_ptr;
    let mut w0_cur = row_ptrs[0];
    let mut w1_cur = row_ptrs[1];
    let mut w2_cur = row_ptrs[2];
    let mut w3_cur = row_ptrs[3];
    let mut remaining = in_features;
    while remaining >= 4 {
        let x = unsafe { v128_load(x_cur as *const v128) };
        let w0 = unsafe { v128_load(w0_cur as *const v128) };
        let w1 = unsafe { v128_load(w1_cur as *const v128) };
        let w2 = unsafe { v128_load(w2_cur as *const v128) };
        let w3 = unsafe { v128_load(w3_cur as *const v128) };
        acc0 = unsafe { f32x4_add(acc0, f32x4_mul(x, w0)) };
        acc1 = unsafe { f32x4_add(acc1, f32x4_mul(x, w1)) };
        acc2 = unsafe { f32x4_add(acc2, f32x4_mul(x, w2)) };
        acc3 = unsafe { f32x4_add(acc3, f32x4_mul(x, w3)) };
        x_cur = unsafe { x_cur.add(16) };
        w0_cur = unsafe { w0_cur.add(16) };
        w1_cur = unsafe { w1_cur.add(16) };
        w2_cur = unsafe { w2_cur.add(16) };
        w3_cur = unsafe { w3_cur.add(16) };
        remaining -= 4;
    }
    let mut sum0 = unsafe { horizontal_sum_f32x4(acc0) };
    let mut sum1 = unsafe { horizontal_sum_f32x4(acc1) };
    let mut sum2 = unsafe { horizontal_sum_f32x4(acc2) };
    let mut sum3 = unsafe { horizontal_sum_f32x4(acc3) };
    while remaining > 0 {
        let x = unsafe { (x_cur as *const f32).read_unaligned() };
        let w0 = unsafe { (w0_cur as *const f32).read_unaligned() };
        let w1 = unsafe { (w1_cur as *const f32).read_unaligned() };
        let w2 = unsafe { (w2_cur as *const f32).read_unaligned() };
        let w3 = unsafe { (w3_cur as *const f32).read_unaligned() };
        sum0 += x * w0;
        sum1 += x * w1;
        sum2 += x * w2;
        sum3 += x * w3;
        x_cur = unsafe { x_cur.add(4) };
        w0_cur = unsafe { w0_cur.add(4) };
        w1_cur = unsafe { w1_cur.add(4) };
        w2_cur = unsafe { w2_cur.add(4) };
        w3_cur = unsafe { w3_cur.add(4) };
        remaining -= 1;
    }
    unsafe {
        (out_ptrs[0] as *mut f32).write_unaligned(sum0);
        (out_ptrs[1] as *mut f32).write_unaligned(sum1);
        (out_ptrs[2] as *mut f32).write_unaligned(sum2);
        (out_ptrs[3] as *mut f32).write_unaligned(sum3);
    }
}

#[cfg(all(target_arch = "aarch64", test))]
#[inline]
unsafe fn linear_dot4_rows_unaligned(
    x_ptr: *const u8,
    x_off: usize,
    w_ptr: *const u8,
    row_offsets: [usize; 4],
    in_features: usize,
) -> [f32; 4] {
    let x_row_ptr = unsafe { x_ptr.add(x_off * 4) };
    let row_ptrs = [
        unsafe { w_ptr.add(row_offsets[0] * 4) },
        unsafe { w_ptr.add(row_offsets[1] * 4) },
        unsafe { w_ptr.add(row_offsets[2] * 4) },
        unsafe { w_ptr.add(row_offsets[3] * 4) },
    ];
    unsafe { linear_dot4_rows_ptrs_unaligned(x_row_ptr, row_ptrs, in_features) }
}

#[cfg(all(target_arch = "aarch64", test))]
#[inline]
unsafe fn linear_dot4_gate_up_interleaved_ptrs_unaligned(
    x_row_ptr: *const u8,
    gate_ptrs: [*const u8; 4],
    up_ptrs: [*const u8; 4],
    in_features: usize,
) -> ([f32; 4], [f32; 4]) {
    use core::arch::aarch64::{vaddvq_f32, vdupq_n_f32, vfmaq_f32};
    let mut gate0_acc = unsafe { vdupq_n_f32(0.0) };
    let mut up0_acc = unsafe { vdupq_n_f32(0.0) };
    let mut gate1_acc = unsafe { vdupq_n_f32(0.0) };
    let mut up1_acc = unsafe { vdupq_n_f32(0.0) };
    let mut gate2_acc = unsafe { vdupq_n_f32(0.0) };
    let mut up2_acc = unsafe { vdupq_n_f32(0.0) };
    let mut gate3_acc = unsafe { vdupq_n_f32(0.0) };
    let mut up3_acc = unsafe { vdupq_n_f32(0.0) };
    let mut x_cur = x_row_ptr;
    let mut gate0_cur = gate_ptrs[0];
    let mut up0_cur = up_ptrs[0];
    let mut gate1_cur = gate_ptrs[1];
    let mut up1_cur = up_ptrs[1];
    let mut gate2_cur = gate_ptrs[2];
    let mut up2_cur = up_ptrs[2];
    let mut gate3_cur = gate_ptrs[3];
    let mut up3_cur = up_ptrs[3];
    let mut remaining = in_features;
    while remaining >= 4 {
        let x = unsafe { load_f32x4_bytes_unaligned(x_cur) };
        gate0_acc = unsafe { vfmaq_f32(gate0_acc, x, load_f32x4_bytes_unaligned(gate0_cur)) };
        up0_acc = unsafe { vfmaq_f32(up0_acc, x, load_f32x4_bytes_unaligned(up0_cur)) };
        gate1_acc = unsafe { vfmaq_f32(gate1_acc, x, load_f32x4_bytes_unaligned(gate1_cur)) };
        up1_acc = unsafe { vfmaq_f32(up1_acc, x, load_f32x4_bytes_unaligned(up1_cur)) };
        gate2_acc = unsafe { vfmaq_f32(gate2_acc, x, load_f32x4_bytes_unaligned(gate2_cur)) };
        up2_acc = unsafe { vfmaq_f32(up2_acc, x, load_f32x4_bytes_unaligned(up2_cur)) };
        gate3_acc = unsafe { vfmaq_f32(gate3_acc, x, load_f32x4_bytes_unaligned(gate3_cur)) };
        up3_acc = unsafe { vfmaq_f32(up3_acc, x, load_f32x4_bytes_unaligned(up3_cur)) };
        x_cur = unsafe { x_cur.add(16) };
        gate0_cur = unsafe { gate0_cur.add(16) };
        up0_cur = unsafe { up0_cur.add(16) };
        gate1_cur = unsafe { gate1_cur.add(16) };
        up1_cur = unsafe { up1_cur.add(16) };
        gate2_cur = unsafe { gate2_cur.add(16) };
        up2_cur = unsafe { up2_cur.add(16) };
        gate3_cur = unsafe { gate3_cur.add(16) };
        up3_cur = unsafe { up3_cur.add(16) };
        remaining -= 4;
    }
    let mut gate0_sum = unsafe { vaddvq_f32(gate0_acc) };
    let mut up0_sum = unsafe { vaddvq_f32(up0_acc) };
    let mut gate1_sum = unsafe { vaddvq_f32(gate1_acc) };
    let mut up1_sum = unsafe { vaddvq_f32(up1_acc) };
    let mut gate2_sum = unsafe { vaddvq_f32(gate2_acc) };
    let mut up2_sum = unsafe { vaddvq_f32(up2_acc) };
    let mut gate3_sum = unsafe { vaddvq_f32(gate3_acc) };
    let mut up3_sum = unsafe { vaddvq_f32(up3_acc) };
    while remaining > 0 {
        let x = unsafe { (x_cur as *const f32).read_unaligned() };
        let gate0_w = unsafe { (gate0_cur as *const f32).read_unaligned() };
        let up0_w = unsafe { (up0_cur as *const f32).read_unaligned() };
        let gate1_w = unsafe { (gate1_cur as *const f32).read_unaligned() };
        let up1_w = unsafe { (up1_cur as *const f32).read_unaligned() };
        let gate2_w = unsafe { (gate2_cur as *const f32).read_unaligned() };
        let up2_w = unsafe { (up2_cur as *const f32).read_unaligned() };
        let gate3_w = unsafe { (gate3_cur as *const f32).read_unaligned() };
        let up3_w = unsafe { (up3_cur as *const f32).read_unaligned() };
        gate0_sum += x * gate0_w;
        up0_sum += x * up0_w;
        gate1_sum += x * gate1_w;
        up1_sum += x * up1_w;
        gate2_sum += x * gate2_w;
        up2_sum += x * up2_w;
        gate3_sum += x * gate3_w;
        up3_sum += x * up3_w;
        x_cur = unsafe { x_cur.add(4) };
        gate0_cur = unsafe { gate0_cur.add(4) };
        up0_cur = unsafe { up0_cur.add(4) };
        gate1_cur = unsafe { gate1_cur.add(4) };
        up1_cur = unsafe { up1_cur.add(4) };
        gate2_cur = unsafe { gate2_cur.add(4) };
        up2_cur = unsafe { up2_cur.add(4) };
        gate3_cur = unsafe { gate3_cur.add(4) };
        up3_cur = unsafe { up3_cur.add(4) };
        remaining -= 1;
    }
    (
        [gate0_sum, gate1_sum, gate2_sum, gate3_sum],
        [up0_sum, up1_sum, up2_sum, up3_sum],
    )
}

#[cfg(target_arch = "aarch64")]
#[inline]
unsafe fn linear_gate_up4_store_ptrs_unaligned(
    x_row_ptr: *const u8,
    gate_ptrs: [*const u8; 4],
    up_ptrs: [*const u8; 4],
    in_features: usize,
    out_ptr: *mut u8,
) {
    use core::arch::aarch64::{vaddvq_f32, vdupq_n_f32, vfmaq_f32};
    let mut gate0_acc = unsafe { vdupq_n_f32(0.0) };
    let mut up0_acc = unsafe { vdupq_n_f32(0.0) };
    let mut gate1_acc = unsafe { vdupq_n_f32(0.0) };
    let mut up1_acc = unsafe { vdupq_n_f32(0.0) };
    let mut gate2_acc = unsafe { vdupq_n_f32(0.0) };
    let mut up2_acc = unsafe { vdupq_n_f32(0.0) };
    let mut gate3_acc = unsafe { vdupq_n_f32(0.0) };
    let mut up3_acc = unsafe { vdupq_n_f32(0.0) };
    let mut x_cur = x_row_ptr;
    let mut gate0_cur = gate_ptrs[0];
    let mut up0_cur = up_ptrs[0];
    let mut gate1_cur = gate_ptrs[1];
    let mut up1_cur = up_ptrs[1];
    let mut gate2_cur = gate_ptrs[2];
    let mut up2_cur = up_ptrs[2];
    let mut gate3_cur = gate_ptrs[3];
    let mut up3_cur = up_ptrs[3];
    let mut remaining = in_features;
    while remaining >= 4 {
        let x = unsafe { load_f32x4_bytes_unaligned(x_cur) };
        gate0_acc = unsafe { vfmaq_f32(gate0_acc, x, load_f32x4_bytes_unaligned(gate0_cur)) };
        up0_acc = unsafe { vfmaq_f32(up0_acc, x, load_f32x4_bytes_unaligned(up0_cur)) };
        gate1_acc = unsafe { vfmaq_f32(gate1_acc, x, load_f32x4_bytes_unaligned(gate1_cur)) };
        up1_acc = unsafe { vfmaq_f32(up1_acc, x, load_f32x4_bytes_unaligned(up1_cur)) };
        gate2_acc = unsafe { vfmaq_f32(gate2_acc, x, load_f32x4_bytes_unaligned(gate2_cur)) };
        up2_acc = unsafe { vfmaq_f32(up2_acc, x, load_f32x4_bytes_unaligned(up2_cur)) };
        gate3_acc = unsafe { vfmaq_f32(gate3_acc, x, load_f32x4_bytes_unaligned(gate3_cur)) };
        up3_acc = unsafe { vfmaq_f32(up3_acc, x, load_f32x4_bytes_unaligned(up3_cur)) };
        x_cur = unsafe { x_cur.add(16) };
        gate0_cur = unsafe { gate0_cur.add(16) };
        up0_cur = unsafe { up0_cur.add(16) };
        gate1_cur = unsafe { gate1_cur.add(16) };
        up1_cur = unsafe { up1_cur.add(16) };
        gate2_cur = unsafe { gate2_cur.add(16) };
        up2_cur = unsafe { up2_cur.add(16) };
        gate3_cur = unsafe { gate3_cur.add(16) };
        up3_cur = unsafe { up3_cur.add(16) };
        remaining -= 4;
    }
    let mut gate0_sum = unsafe { vaddvq_f32(gate0_acc) };
    let mut up0_sum = unsafe { vaddvq_f32(up0_acc) };
    let mut gate1_sum = unsafe { vaddvq_f32(gate1_acc) };
    let mut up1_sum = unsafe { vaddvq_f32(up1_acc) };
    let mut gate2_sum = unsafe { vaddvq_f32(gate2_acc) };
    let mut up2_sum = unsafe { vaddvq_f32(up2_acc) };
    let mut gate3_sum = unsafe { vaddvq_f32(gate3_acc) };
    let mut up3_sum = unsafe { vaddvq_f32(up3_acc) };
    while remaining > 0 {
        let x = unsafe { load_f32_bytes_unaligned(x_cur) };
        let gate0_w = unsafe { load_f32_bytes_unaligned(gate0_cur) };
        let up0_w = unsafe { load_f32_bytes_unaligned(up0_cur) };
        let gate1_w = unsafe { load_f32_bytes_unaligned(gate1_cur) };
        let up1_w = unsafe { load_f32_bytes_unaligned(up1_cur) };
        let gate2_w = unsafe { load_f32_bytes_unaligned(gate2_cur) };
        let up2_w = unsafe { load_f32_bytes_unaligned(up2_cur) };
        let gate3_w = unsafe { load_f32_bytes_unaligned(gate3_cur) };
        let up3_w = unsafe { load_f32_bytes_unaligned(up3_cur) };
        gate0_sum += x * gate0_w;
        up0_sum += x * up0_w;
        gate1_sum += x * gate1_w;
        up1_sum += x * up1_w;
        gate2_sum += x * gate2_w;
        up2_sum += x * up2_w;
        gate3_sum += x * gate3_w;
        up3_sum += x * up3_w;
        x_cur = unsafe { x_cur.add(4) };
        gate0_cur = unsafe { gate0_cur.add(4) };
        up0_cur = unsafe { up0_cur.add(4) };
        gate1_cur = unsafe { gate1_cur.add(4) };
        up1_cur = unsafe { up1_cur.add(4) };
        gate2_cur = unsafe { gate2_cur.add(4) };
        up2_cur = unsafe { up2_cur.add(4) };
        gate3_cur = unsafe { gate3_cur.add(4) };
        up3_cur = unsafe { up3_cur.add(4) };
        remaining -= 1;
    }
    let relu0 = gate0_sum.max(0.0);
    let relu1 = gate1_sum.max(0.0);
    let relu2 = gate2_sum.max(0.0);
    let relu3 = gate3_sum.max(0.0);
    unsafe {
        store_f32_bytes_unaligned(out_ptr, relu0 * relu0 * up0_sum);
        store_f32_bytes_unaligned(out_ptr.add(4), relu1 * relu1 * up1_sum);
        store_f32_bytes_unaligned(out_ptr.add(8), relu2 * relu2 * up2_sum);
        store_f32_bytes_unaligned(out_ptr.add(12), relu3 * relu3 * up3_sum);
    }
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn linear_gate_up4_store_ptrs_unaligned(
    x_row_ptr: *const u8,
    gate_ptrs: [*const u8; 4],
    up_ptrs: [*const u8; 4],
    in_features: usize,
    out_ptr: *mut u8,
) {
    use std::arch::x86_64::*;
    let mut gate0_acc = unsafe { _mm_setzero_ps() };
    let mut up0_acc = unsafe { _mm_setzero_ps() };
    let mut gate1_acc = unsafe { _mm_setzero_ps() };
    let mut up1_acc = unsafe { _mm_setzero_ps() };
    let mut gate2_acc = unsafe { _mm_setzero_ps() };
    let mut up2_acc = unsafe { _mm_setzero_ps() };
    let mut gate3_acc = unsafe { _mm_setzero_ps() };
    let mut up3_acc = unsafe { _mm_setzero_ps() };
    let mut x_cur = x_row_ptr;
    let mut gate0_cur = gate_ptrs[0];
    let mut up0_cur = up_ptrs[0];
    let mut gate1_cur = gate_ptrs[1];
    let mut up1_cur = up_ptrs[1];
    let mut gate2_cur = gate_ptrs[2];
    let mut up2_cur = up_ptrs[2];
    let mut gate3_cur = gate_ptrs[3];
    let mut up3_cur = up_ptrs[3];
    let mut remaining = in_features;
    while remaining >= 4 {
        let x = unsafe { _mm_loadu_ps(x_cur as *const f32) };
        gate0_acc = unsafe {
            _mm_add_ps(
                gate0_acc,
                _mm_mul_ps(x, _mm_loadu_ps(gate0_cur as *const f32)),
            )
        };
        up0_acc =
            unsafe { _mm_add_ps(up0_acc, _mm_mul_ps(x, _mm_loadu_ps(up0_cur as *const f32))) };
        gate1_acc = unsafe {
            _mm_add_ps(
                gate1_acc,
                _mm_mul_ps(x, _mm_loadu_ps(gate1_cur as *const f32)),
            )
        };
        up1_acc =
            unsafe { _mm_add_ps(up1_acc, _mm_mul_ps(x, _mm_loadu_ps(up1_cur as *const f32))) };
        gate2_acc = unsafe {
            _mm_add_ps(
                gate2_acc,
                _mm_mul_ps(x, _mm_loadu_ps(gate2_cur as *const f32)),
            )
        };
        up2_acc =
            unsafe { _mm_add_ps(up2_acc, _mm_mul_ps(x, _mm_loadu_ps(up2_cur as *const f32))) };
        gate3_acc = unsafe {
            _mm_add_ps(
                gate3_acc,
                _mm_mul_ps(x, _mm_loadu_ps(gate3_cur as *const f32)),
            )
        };
        up3_acc =
            unsafe { _mm_add_ps(up3_acc, _mm_mul_ps(x, _mm_loadu_ps(up3_cur as *const f32))) };
        x_cur = unsafe { x_cur.add(16) };
        gate0_cur = unsafe { gate0_cur.add(16) };
        up0_cur = unsafe { up0_cur.add(16) };
        gate1_cur = unsafe { gate1_cur.add(16) };
        up1_cur = unsafe { up1_cur.add(16) };
        gate2_cur = unsafe { gate2_cur.add(16) };
        up2_cur = unsafe { up2_cur.add(16) };
        gate3_cur = unsafe { gate3_cur.add(16) };
        up3_cur = unsafe { up3_cur.add(16) };
        remaining -= 4;
    }
    let mut gate0_sum = unsafe { horizontal_sum_f32x4(gate0_acc) };
    let mut up0_sum = unsafe { horizontal_sum_f32x4(up0_acc) };
    let mut gate1_sum = unsafe { horizontal_sum_f32x4(gate1_acc) };
    let mut up1_sum = unsafe { horizontal_sum_f32x4(up1_acc) };
    let mut gate2_sum = unsafe { horizontal_sum_f32x4(gate2_acc) };
    let mut up2_sum = unsafe { horizontal_sum_f32x4(up2_acc) };
    let mut gate3_sum = unsafe { horizontal_sum_f32x4(gate3_acc) };
    let mut up3_sum = unsafe { horizontal_sum_f32x4(up3_acc) };
    while remaining > 0 {
        let x = unsafe { (x_cur as *const f32).read_unaligned() };
        let gate0_w = unsafe { (gate0_cur as *const f32).read_unaligned() };
        let up0_w = unsafe { (up0_cur as *const f32).read_unaligned() };
        let gate1_w = unsafe { (gate1_cur as *const f32).read_unaligned() };
        let up1_w = unsafe { (up1_cur as *const f32).read_unaligned() };
        let gate2_w = unsafe { (gate2_cur as *const f32).read_unaligned() };
        let up2_w = unsafe { (up2_cur as *const f32).read_unaligned() };
        let gate3_w = unsafe { (gate3_cur as *const f32).read_unaligned() };
        let up3_w = unsafe { (up3_cur as *const f32).read_unaligned() };
        gate0_sum += x * gate0_w;
        up0_sum += x * up0_w;
        gate1_sum += x * gate1_w;
        up1_sum += x * up1_w;
        gate2_sum += x * gate2_w;
        up2_sum += x * up2_w;
        gate3_sum += x * gate3_w;
        up3_sum += x * up3_w;
        x_cur = unsafe { x_cur.add(4) };
        gate0_cur = unsafe { gate0_cur.add(4) };
        up0_cur = unsafe { up0_cur.add(4) };
        gate1_cur = unsafe { gate1_cur.add(4) };
        up1_cur = unsafe { up1_cur.add(4) };
        gate2_cur = unsafe { gate2_cur.add(4) };
        up2_cur = unsafe { up2_cur.add(4) };
        gate3_cur = unsafe { gate3_cur.add(4) };
        up3_cur = unsafe { up3_cur.add(4) };
        remaining -= 1;
    }
    let relu0 = gate0_sum.max(0.0);
    let relu1 = gate1_sum.max(0.0);
    let relu2 = gate2_sum.max(0.0);
    let relu3 = gate3_sum.max(0.0);
    unsafe {
        (out_ptr as *mut f32).write_unaligned(relu0 * relu0 * up0_sum);
        (out_ptr.add(4) as *mut f32).write_unaligned(relu1 * relu1 * up1_sum);
        (out_ptr.add(8) as *mut f32).write_unaligned(relu2 * relu2 * up2_sum);
        (out_ptr.add(12) as *mut f32).write_unaligned(relu3 * relu3 * up3_sum);
    }
}

#[cfg(all(target_arch = "wasm32", target_feature = "simd128"))]
#[inline]
unsafe fn linear_gate_up4_store_ptrs_unaligned(
    x_row_ptr: *const u8,
    gate_ptrs: [*const u8; 4],
    up_ptrs: [*const u8; 4],
    in_features: usize,
    out_ptr: *mut u8,
) {
    use std::arch::wasm32::*;
    let mut gate0_acc = unsafe { f32x4_splat(0.0) };
    let mut up0_acc = unsafe { f32x4_splat(0.0) };
    let mut gate1_acc = unsafe { f32x4_splat(0.0) };
    let mut up1_acc = unsafe { f32x4_splat(0.0) };
    let mut gate2_acc = unsafe { f32x4_splat(0.0) };
    let mut up2_acc = unsafe { f32x4_splat(0.0) };
    let mut gate3_acc = unsafe { f32x4_splat(0.0) };
    let mut up3_acc = unsafe { f32x4_splat(0.0) };
    let mut x_cur = x_row_ptr;
    let mut gate0_cur = gate_ptrs[0];
    let mut up0_cur = up_ptrs[0];
    let mut gate1_cur = gate_ptrs[1];
    let mut up1_cur = up_ptrs[1];
    let mut gate2_cur = gate_ptrs[2];
    let mut up2_cur = up_ptrs[2];
    let mut gate3_cur = gate_ptrs[3];
    let mut up3_cur = up_ptrs[3];
    let mut remaining = in_features;
    while remaining >= 4 {
        let x = unsafe { v128_load(x_cur as *const v128) };
        gate0_acc =
            unsafe { f32x4_add(gate0_acc, f32x4_mul(x, v128_load(gate0_cur as *const v128))) };
        up0_acc = unsafe { f32x4_add(up0_acc, f32x4_mul(x, v128_load(up0_cur as *const v128))) };
        gate1_acc =
            unsafe { f32x4_add(gate1_acc, f32x4_mul(x, v128_load(gate1_cur as *const v128))) };
        up1_acc = unsafe { f32x4_add(up1_acc, f32x4_mul(x, v128_load(up1_cur as *const v128))) };
        gate2_acc =
            unsafe { f32x4_add(gate2_acc, f32x4_mul(x, v128_load(gate2_cur as *const v128))) };
        up2_acc = unsafe { f32x4_add(up2_acc, f32x4_mul(x, v128_load(up2_cur as *const v128))) };
        gate3_acc =
            unsafe { f32x4_add(gate3_acc, f32x4_mul(x, v128_load(gate3_cur as *const v128))) };
        up3_acc = unsafe { f32x4_add(up3_acc, f32x4_mul(x, v128_load(up3_cur as *const v128))) };
        x_cur = unsafe { x_cur.add(16) };
        gate0_cur = unsafe { gate0_cur.add(16) };
        up0_cur = unsafe { up0_cur.add(16) };
        gate1_cur = unsafe { gate1_cur.add(16) };
        up1_cur = unsafe { up1_cur.add(16) };
        gate2_cur = unsafe { gate2_cur.add(16) };
        up2_cur = unsafe { up2_cur.add(16) };
        gate3_cur = unsafe { gate3_cur.add(16) };
        up3_cur = unsafe { up3_cur.add(16) };
        remaining -= 4;
    }
    let mut gate0_sum = unsafe { horizontal_sum_f32x4(gate0_acc) };
    let mut up0_sum = unsafe { horizontal_sum_f32x4(up0_acc) };
    let mut gate1_sum = unsafe { horizontal_sum_f32x4(gate1_acc) };
    let mut up1_sum = unsafe { horizontal_sum_f32x4(up1_acc) };
    let mut gate2_sum = unsafe { horizontal_sum_f32x4(gate2_acc) };
    let mut up2_sum = unsafe { horizontal_sum_f32x4(up2_acc) };
    let mut gate3_sum = unsafe { horizontal_sum_f32x4(gate3_acc) };
    let mut up3_sum = unsafe { horizontal_sum_f32x4(up3_acc) };
    while remaining > 0 {
        let x = unsafe { (x_cur as *const f32).read_unaligned() };
        let gate0_w = unsafe { (gate0_cur as *const f32).read_unaligned() };
        let up0_w = unsafe { (up0_cur as *const f32).read_unaligned() };
        let gate1_w = unsafe { (gate1_cur as *const f32).read_unaligned() };
        let up1_w = unsafe { (up1_cur as *const f32).read_unaligned() };
        let gate2_w = unsafe { (gate2_cur as *const f32).read_unaligned() };
        let up2_w = unsafe { (up2_cur as *const f32).read_unaligned() };
        let gate3_w = unsafe { (gate3_cur as *const f32).read_unaligned() };
        let up3_w = unsafe { (up3_cur as *const f32).read_unaligned() };
        gate0_sum += x * gate0_w;
        up0_sum += x * up0_w;
        gate1_sum += x * gate1_w;
        up1_sum += x * up1_w;
        gate2_sum += x * gate2_w;
        up2_sum += x * up2_w;
        gate3_sum += x * gate3_w;
        up3_sum += x * up3_w;
        x_cur = unsafe { x_cur.add(4) };
        gate0_cur = unsafe { gate0_cur.add(4) };
        up0_cur = unsafe { up0_cur.add(4) };
        gate1_cur = unsafe { gate1_cur.add(4) };
        up1_cur = unsafe { up1_cur.add(4) };
        gate2_cur = unsafe { gate2_cur.add(4) };
        up2_cur = unsafe { up2_cur.add(4) };
        gate3_cur = unsafe { gate3_cur.add(4) };
        up3_cur = unsafe { up3_cur.add(4) };
        remaining -= 1;
    }
    let relu0 = gate0_sum.max(0.0);
    let relu1 = gate1_sum.max(0.0);
    let relu2 = gate2_sum.max(0.0);
    let relu3 = gate3_sum.max(0.0);
    unsafe {
        (out_ptr as *mut f32).write_unaligned(relu0 * relu0 * up0_sum);
        (out_ptr.add(4) as *mut f32).write_unaligned(relu1 * relu1 * up1_sum);
        (out_ptr.add(8) as *mut f32).write_unaligned(relu2 * relu2 * up2_sum);
        (out_ptr.add(12) as *mut f32).write_unaligned(relu3 * relu3 * up3_sum);
    }
}

#[cfg(all(
    any(
        target_arch = "aarch64",
        target_arch = "x86_64",
        all(target_arch = "wasm32", target_feature = "simd128")
    ),
    test
))]
#[inline]
unsafe fn linear_gate_up4_store_unaligned(
    x_ptr: *const u8,
    x_off: usize,
    weight_ptr: *const u8,
    hidden_idx: usize,
    in_features: usize,
    out_ptr: *mut u8,
) {
    let gate0_off = (2 * hidden_idx) * in_features;
    let up0_off = (2 * hidden_idx + 1) * in_features;
    let gate1_off = (2 * (hidden_idx + 1)) * in_features;
    let up1_off = (2 * (hidden_idx + 1) + 1) * in_features;
    let gate2_off = (2 * (hidden_idx + 2)) * in_features;
    let up2_off = (2 * (hidden_idx + 2) + 1) * in_features;
    let gate3_off = (2 * (hidden_idx + 3)) * in_features;
    let up3_off = (2 * (hidden_idx + 3) + 1) * in_features;
    let x_row_ptr = unsafe { x_ptr.add(x_off * 4) };
    unsafe {
        linear_gate_up4_store_ptrs_unaligned(
            x_row_ptr,
            [
                weight_ptr.add(gate0_off * 4),
                weight_ptr.add(gate1_off * 4),
                weight_ptr.add(gate2_off * 4),
                weight_ptr.add(gate3_off * 4),
            ],
            [
                weight_ptr.add(up0_off * 4),
                weight_ptr.add(up1_off * 4),
                weight_ptr.add(up2_off * 4),
                weight_ptr.add(up3_off * 4),
            ],
            in_features,
            out_ptr,
        );
    }
}

#[cfg(all(target_arch = "aarch64", test))]
#[inline]
unsafe fn linear_dot4_gate_up_interleaved_unaligned(
    x_ptr: *const u8,
    x_off: usize,
    weight_ptr: *const u8,
    hidden_idx: usize,
    in_features: usize,
) -> ([f32; 4], [f32; 4]) {
    let gate0_off = (2 * hidden_idx) * in_features;
    let up0_off = (2 * hidden_idx + 1) * in_features;
    let gate1_off = (2 * (hidden_idx + 1)) * in_features;
    let up1_off = (2 * (hidden_idx + 1) + 1) * in_features;
    let gate2_off = (2 * (hidden_idx + 2)) * in_features;
    let up2_off = (2 * (hidden_idx + 2) + 1) * in_features;
    let gate3_off = (2 * (hidden_idx + 3)) * in_features;
    let up3_off = (2 * (hidden_idx + 3) + 1) * in_features;
    let x_row_ptr = unsafe { x_ptr.add(x_off * 4) };
    let gate_ptrs = [
        unsafe { weight_ptr.add(gate0_off * 4) },
        unsafe { weight_ptr.add(gate1_off * 4) },
        unsafe { weight_ptr.add(gate2_off * 4) },
        unsafe { weight_ptr.add(gate3_off * 4) },
    ];
    let up_ptrs = [
        unsafe { weight_ptr.add(up0_off * 4) },
        unsafe { weight_ptr.add(up1_off * 4) },
        unsafe { weight_ptr.add(up2_off * 4) },
        unsafe { weight_ptr.add(up3_off * 4) },
    ];
    unsafe {
        linear_dot4_gate_up_interleaved_ptrs_unaligned(x_row_ptr, gate_ptrs, up_ptrs, in_features)
    }
}

#[cfg(target_arch = "aarch64")]
#[cfg(target_arch = "aarch64")]
#[cfg(target_arch = "aarch64")]
unsafe fn linear_gate_up8_store_group_unaligned(
    x_row_ptr: *const u8,
    weight_group_ptr: *const u8,
    row_stride_bytes: usize,
    in_features: usize,
    out_ptr: *mut u8,
) {
    let gate0 = weight_group_ptr;
    let up0 = unsafe { gate0.add(row_stride_bytes) };
    let gate1 = unsafe { up0.add(row_stride_bytes) };
    let up1 = unsafe { gate1.add(row_stride_bytes) };
    let gate2 = unsafe { up1.add(row_stride_bytes) };
    let up2 = unsafe { gate2.add(row_stride_bytes) };
    let gate3 = unsafe { up2.add(row_stride_bytes) };
    let up3 = unsafe { gate3.add(row_stride_bytes) };
    let gate4 = unsafe { up3.add(row_stride_bytes) };
    let up4 = unsafe { gate4.add(row_stride_bytes) };
    let gate5 = unsafe { up4.add(row_stride_bytes) };
    let up5 = unsafe { gate5.add(row_stride_bytes) };
    let gate6 = unsafe { up5.add(row_stride_bytes) };
    let up6 = unsafe { gate6.add(row_stride_bytes) };
    let gate7 = unsafe { up6.add(row_stride_bytes) };
    let up7 = unsafe { gate7.add(row_stride_bytes) };
    use core::arch::aarch64::{vaddvq_f32, vdupq_n_f32, vfmaq_f32};
    let mut gate0_acc = unsafe { vdupq_n_f32(0.0) };
    let mut up0_acc = unsafe { vdupq_n_f32(0.0) };
    let mut gate1_acc = unsafe { vdupq_n_f32(0.0) };
    let mut up1_acc = unsafe { vdupq_n_f32(0.0) };
    let mut gate2_acc = unsafe { vdupq_n_f32(0.0) };
    let mut up2_acc = unsafe { vdupq_n_f32(0.0) };
    let mut gate3_acc = unsafe { vdupq_n_f32(0.0) };
    let mut up3_acc = unsafe { vdupq_n_f32(0.0) };
    let mut gate4_acc = unsafe { vdupq_n_f32(0.0) };
    let mut up4_acc = unsafe { vdupq_n_f32(0.0) };
    let mut gate5_acc = unsafe { vdupq_n_f32(0.0) };
    let mut up5_acc = unsafe { vdupq_n_f32(0.0) };
    let mut gate6_acc = unsafe { vdupq_n_f32(0.0) };
    let mut up6_acc = unsafe { vdupq_n_f32(0.0) };
    let mut gate7_acc = unsafe { vdupq_n_f32(0.0) };
    let mut up7_acc = unsafe { vdupq_n_f32(0.0) };
    let mut x_cur = x_row_ptr;
    let mut gate0_cur = gate0;
    let mut up0_cur = up0;
    let mut gate1_cur = gate1;
    let mut up1_cur = up1;
    let mut gate2_cur = gate2;
    let mut up2_cur = up2;
    let mut gate3_cur = gate3;
    let mut up3_cur = up3;
    let mut gate4_cur = gate4;
    let mut up4_cur = up4;
    let mut gate5_cur = gate5;
    let mut up5_cur = up5;
    let mut gate6_cur = gate6;
    let mut up6_cur = up6;
    let mut gate7_cur = gate7;
    let mut up7_cur = up7;
    let mut remaining = in_features;
    while remaining >= 4 {
        let x = unsafe { load_f32x4_bytes_unaligned(x_cur) };
        gate0_acc = unsafe { vfmaq_f32(gate0_acc, x, load_f32x4_bytes_unaligned(gate0_cur)) };
        up0_acc = unsafe { vfmaq_f32(up0_acc, x, load_f32x4_bytes_unaligned(up0_cur)) };
        gate1_acc = unsafe { vfmaq_f32(gate1_acc, x, load_f32x4_bytes_unaligned(gate1_cur)) };
        up1_acc = unsafe { vfmaq_f32(up1_acc, x, load_f32x4_bytes_unaligned(up1_cur)) };
        gate2_acc = unsafe { vfmaq_f32(gate2_acc, x, load_f32x4_bytes_unaligned(gate2_cur)) };
        up2_acc = unsafe { vfmaq_f32(up2_acc, x, load_f32x4_bytes_unaligned(up2_cur)) };
        gate3_acc = unsafe { vfmaq_f32(gate3_acc, x, load_f32x4_bytes_unaligned(gate3_cur)) };
        up3_acc = unsafe { vfmaq_f32(up3_acc, x, load_f32x4_bytes_unaligned(up3_cur)) };
        gate4_acc = unsafe { vfmaq_f32(gate4_acc, x, load_f32x4_bytes_unaligned(gate4_cur)) };
        up4_acc = unsafe { vfmaq_f32(up4_acc, x, load_f32x4_bytes_unaligned(up4_cur)) };
        gate5_acc = unsafe { vfmaq_f32(gate5_acc, x, load_f32x4_bytes_unaligned(gate5_cur)) };
        up5_acc = unsafe { vfmaq_f32(up5_acc, x, load_f32x4_bytes_unaligned(up5_cur)) };
        gate6_acc = unsafe { vfmaq_f32(gate6_acc, x, load_f32x4_bytes_unaligned(gate6_cur)) };
        up6_acc = unsafe { vfmaq_f32(up6_acc, x, load_f32x4_bytes_unaligned(up6_cur)) };
        gate7_acc = unsafe { vfmaq_f32(gate7_acc, x, load_f32x4_bytes_unaligned(gate7_cur)) };
        up7_acc = unsafe { vfmaq_f32(up7_acc, x, load_f32x4_bytes_unaligned(up7_cur)) };
        gate0_cur = unsafe { gate0_cur.add(16) };
        up0_cur = unsafe { up0_cur.add(16) };
        gate1_cur = unsafe { gate1_cur.add(16) };
        up1_cur = unsafe { up1_cur.add(16) };
        gate2_cur = unsafe { gate2_cur.add(16) };
        up2_cur = unsafe { up2_cur.add(16) };
        gate3_cur = unsafe { gate3_cur.add(16) };
        up3_cur = unsafe { up3_cur.add(16) };
        gate4_cur = unsafe { gate4_cur.add(16) };
        up4_cur = unsafe { up4_cur.add(16) };
        gate5_cur = unsafe { gate5_cur.add(16) };
        up5_cur = unsafe { up5_cur.add(16) };
        gate6_cur = unsafe { gate6_cur.add(16) };
        up6_cur = unsafe { up6_cur.add(16) };
        gate7_cur = unsafe { gate7_cur.add(16) };
        up7_cur = unsafe { up7_cur.add(16) };
        x_cur = unsafe { x_cur.add(16) };
        remaining -= 4;
    }
    let mut gate0_sum = unsafe { vaddvq_f32(gate0_acc) };
    let mut up0_sum = unsafe { vaddvq_f32(up0_acc) };
    let mut gate1_sum = unsafe { vaddvq_f32(gate1_acc) };
    let mut up1_sum = unsafe { vaddvq_f32(up1_acc) };
    let mut gate2_sum = unsafe { vaddvq_f32(gate2_acc) };
    let mut up2_sum = unsafe { vaddvq_f32(up2_acc) };
    let mut gate3_sum = unsafe { vaddvq_f32(gate3_acc) };
    let mut up3_sum = unsafe { vaddvq_f32(up3_acc) };
    let mut gate4_sum = unsafe { vaddvq_f32(gate4_acc) };
    let mut up4_sum = unsafe { vaddvq_f32(up4_acc) };
    let mut gate5_sum = unsafe { vaddvq_f32(gate5_acc) };
    let mut up5_sum = unsafe { vaddvq_f32(up5_acc) };
    let mut gate6_sum = unsafe { vaddvq_f32(gate6_acc) };
    let mut up6_sum = unsafe { vaddvq_f32(up6_acc) };
    let mut gate7_sum = unsafe { vaddvq_f32(gate7_acc) };
    let mut up7_sum = unsafe { vaddvq_f32(up7_acc) };
    while remaining > 0 {
        let x = unsafe { load_f32_bytes_unaligned(x_cur) };
        let gate0_w = unsafe { load_f32_bytes_unaligned(gate0_cur) };
        let up0_w = unsafe { load_f32_bytes_unaligned(up0_cur) };
        let gate1_w = unsafe { load_f32_bytes_unaligned(gate1_cur) };
        let up1_w = unsafe { load_f32_bytes_unaligned(up1_cur) };
        let gate2_w = unsafe { load_f32_bytes_unaligned(gate2_cur) };
        let up2_w = unsafe { load_f32_bytes_unaligned(up2_cur) };
        let gate3_w = unsafe { load_f32_bytes_unaligned(gate3_cur) };
        let up3_w = unsafe { load_f32_bytes_unaligned(up3_cur) };
        let gate4_w = unsafe { load_f32_bytes_unaligned(gate4_cur) };
        let up4_w = unsafe { load_f32_bytes_unaligned(up4_cur) };
        let gate5_w = unsafe { load_f32_bytes_unaligned(gate5_cur) };
        let up5_w = unsafe { load_f32_bytes_unaligned(up5_cur) };
        let gate6_w = unsafe { load_f32_bytes_unaligned(gate6_cur) };
        let up6_w = unsafe { load_f32_bytes_unaligned(up6_cur) };
        let gate7_w = unsafe { load_f32_bytes_unaligned(gate7_cur) };
        let up7_w = unsafe { load_f32_bytes_unaligned(up7_cur) };
        gate0_sum += x * gate0_w;
        up0_sum += x * up0_w;
        gate1_sum += x * gate1_w;
        up1_sum += x * up1_w;
        gate2_sum += x * gate2_w;
        up2_sum += x * up2_w;
        gate3_sum += x * gate3_w;
        up3_sum += x * up3_w;
        gate4_sum += x * gate4_w;
        up4_sum += x * up4_w;
        gate5_sum += x * gate5_w;
        up5_sum += x * up5_w;
        gate6_sum += x * gate6_w;
        up6_sum += x * up6_w;
        gate7_sum += x * gate7_w;
        up7_sum += x * up7_w;
        gate0_cur = unsafe { gate0_cur.add(4) };
        up0_cur = unsafe { up0_cur.add(4) };
        gate1_cur = unsafe { gate1_cur.add(4) };
        up1_cur = unsafe { up1_cur.add(4) };
        gate2_cur = unsafe { gate2_cur.add(4) };
        up2_cur = unsafe { up2_cur.add(4) };
        gate3_cur = unsafe { gate3_cur.add(4) };
        up3_cur = unsafe { up3_cur.add(4) };
        gate4_cur = unsafe { gate4_cur.add(4) };
        up4_cur = unsafe { up4_cur.add(4) };
        gate5_cur = unsafe { gate5_cur.add(4) };
        up5_cur = unsafe { up5_cur.add(4) };
        gate6_cur = unsafe { gate6_cur.add(4) };
        up6_cur = unsafe { up6_cur.add(4) };
        gate7_cur = unsafe { gate7_cur.add(4) };
        up7_cur = unsafe { up7_cur.add(4) };
        x_cur = unsafe { x_cur.add(4) };
        remaining -= 1;
    }
    let relu0 = gate0_sum.max(0.0);
    let relu1 = gate1_sum.max(0.0);
    let relu2 = gate2_sum.max(0.0);
    let relu3 = gate3_sum.max(0.0);
    let relu4 = gate4_sum.max(0.0);
    let relu5 = gate5_sum.max(0.0);
    let relu6 = gate6_sum.max(0.0);
    let relu7 = gate7_sum.max(0.0);
    unsafe {
        store_f32_bytes_unaligned(out_ptr, relu0 * relu0 * up0_sum);
        store_f32_bytes_unaligned(out_ptr.add(4), relu1 * relu1 * up1_sum);
        store_f32_bytes_unaligned(out_ptr.add(8), relu2 * relu2 * up2_sum);
        store_f32_bytes_unaligned(out_ptr.add(12), relu3 * relu3 * up3_sum);
        store_f32_bytes_unaligned(out_ptr.add(16), relu4 * relu4 * up4_sum);
        store_f32_bytes_unaligned(out_ptr.add(20), relu5 * relu5 * up5_sum);
        store_f32_bytes_unaligned(out_ptr.add(24), relu6 * relu6 * up6_sum);
        store_f32_bytes_unaligned(out_ptr.add(28), relu7 * relu7 * up7_sum);
    }
}

#[cfg(all(target_arch = "aarch64", test))]
#[inline]
unsafe fn linear_gate_up8_store_unaligned(
    x_ptr: *const u8,
    x_off: usize,
    weight_ptr: *const u8,
    hidden_idx: usize,
    in_features: usize,
    out_ptr: *mut u8,
) {
    let x_row_ptr = unsafe { x_ptr.add(x_off * 4) };
    let row_stride_bytes = in_features * 4;
    let pair_stride_bytes = row_stride_bytes * 2;
    let weight_group_ptr = unsafe { weight_ptr.add(hidden_idx * pair_stride_bytes) };
    unsafe {
        linear_gate_up8_store_group_unaligned(
            x_row_ptr,
            weight_group_ptr,
            row_stride_bytes,
            in_features,
            out_ptr,
        );
    }
}

#[cfg(all(target_arch = "aarch64", test))]
#[inline]
unsafe fn linear_dot8_gate_up_interleaved_ptrs_unaligned(
    x_row_ptr: *const u8,
    gate_ptrs: [*const u8; 8],
    up_ptrs: [*const u8; 8],
    in_features: usize,
) -> ([f32; 8], [f32; 8]) {
    use core::arch::aarch64::{vaddvq_f32, vdupq_n_f32, vfmaq_f32};

    let mut gate_acc = [unsafe { vdupq_n_f32(0.0) }; 8];
    let mut up_acc = [unsafe { vdupq_n_f32(0.0) }; 8];
    let mut x_cur = x_row_ptr;
    let mut gate_cur = gate_ptrs;
    let mut up_cur = up_ptrs;
    let mut remaining = in_features;
    while remaining >= 4 {
        let x = unsafe { load_f32x4_bytes_unaligned(x_cur) };
        let mut i = 0usize;
        while i < 8 {
            gate_acc[i] =
                unsafe { vfmaq_f32(gate_acc[i], x, load_f32x4_bytes_unaligned(gate_cur[i])) };
            up_acc[i] = unsafe { vfmaq_f32(up_acc[i], x, load_f32x4_bytes_unaligned(up_cur[i])) };
            gate_cur[i] = unsafe { gate_cur[i].add(16) };
            up_cur[i] = unsafe { up_cur[i].add(16) };
            i += 1;
        }
        x_cur = unsafe { x_cur.add(16) };
        remaining -= 4;
    }

    let mut gate_sum = [
        unsafe { vaddvq_f32(gate_acc[0]) },
        unsafe { vaddvq_f32(gate_acc[1]) },
        unsafe { vaddvq_f32(gate_acc[2]) },
        unsafe { vaddvq_f32(gate_acc[3]) },
        unsafe { vaddvq_f32(gate_acc[4]) },
        unsafe { vaddvq_f32(gate_acc[5]) },
        unsafe { vaddvq_f32(gate_acc[6]) },
        unsafe { vaddvq_f32(gate_acc[7]) },
    ];
    let mut up_sum = [
        unsafe { vaddvq_f32(up_acc[0]) },
        unsafe { vaddvq_f32(up_acc[1]) },
        unsafe { vaddvq_f32(up_acc[2]) },
        unsafe { vaddvq_f32(up_acc[3]) },
        unsafe { vaddvq_f32(up_acc[4]) },
        unsafe { vaddvq_f32(up_acc[5]) },
        unsafe { vaddvq_f32(up_acc[6]) },
        unsafe { vaddvq_f32(up_acc[7]) },
    ];
    while remaining > 0 {
        let x = unsafe { (x_cur as *const f32).read_unaligned() };
        let mut i = 0usize;
        while i < 8 {
            let gate_w = unsafe { (gate_cur[i] as *const f32).read_unaligned() };
            let up_w = unsafe { (up_cur[i] as *const f32).read_unaligned() };
            gate_sum[i] += x * gate_w;
            up_sum[i] += x * up_w;
            gate_cur[i] = unsafe { gate_cur[i].add(4) };
            up_cur[i] = unsafe { up_cur[i].add(4) };
            i += 1;
        }
        x_cur = unsafe { x_cur.add(4) };
        remaining -= 1;
    }
    (gate_sum, up_sum)
}

#[cfg(all(target_arch = "aarch64", test))]
#[inline]
unsafe fn linear_dot8_gate_up_interleaved_unaligned(
    x_ptr: *const u8,
    x_off: usize,
    weight_ptr: *const u8,
    hidden_idx: usize,
    in_features: usize,
) -> ([f32; 8], [f32; 8]) {
    let gate0_off = (2 * hidden_idx) * in_features;
    let up0_off = (2 * hidden_idx + 1) * in_features;
    let gate1_off = (2 * (hidden_idx + 1)) * in_features;
    let up1_off = (2 * (hidden_idx + 1) + 1) * in_features;
    let gate2_off = (2 * (hidden_idx + 2)) * in_features;
    let up2_off = (2 * (hidden_idx + 2) + 1) * in_features;
    let gate3_off = (2 * (hidden_idx + 3)) * in_features;
    let up3_off = (2 * (hidden_idx + 3) + 1) * in_features;
    let gate4_off = (2 * (hidden_idx + 4)) * in_features;
    let up4_off = (2 * (hidden_idx + 4) + 1) * in_features;
    let gate5_off = (2 * (hidden_idx + 5)) * in_features;
    let up5_off = (2 * (hidden_idx + 5) + 1) * in_features;
    let gate6_off = (2 * (hidden_idx + 6)) * in_features;
    let up6_off = (2 * (hidden_idx + 6) + 1) * in_features;
    let gate7_off = (2 * (hidden_idx + 7)) * in_features;
    let up7_off = (2 * (hidden_idx + 7) + 1) * in_features;
    let x_row_ptr = unsafe { x_ptr.add(x_off * 4) };
    let gate_ptrs = [
        unsafe { weight_ptr.add(gate0_off * 4) },
        unsafe { weight_ptr.add(gate1_off * 4) },
        unsafe { weight_ptr.add(gate2_off * 4) },
        unsafe { weight_ptr.add(gate3_off * 4) },
        unsafe { weight_ptr.add(gate4_off * 4) },
        unsafe { weight_ptr.add(gate5_off * 4) },
        unsafe { weight_ptr.add(gate6_off * 4) },
        unsafe { weight_ptr.add(gate7_off * 4) },
    ];
    let up_ptrs = [
        unsafe { weight_ptr.add(up0_off * 4) },
        unsafe { weight_ptr.add(up1_off * 4) },
        unsafe { weight_ptr.add(up2_off * 4) },
        unsafe { weight_ptr.add(up3_off * 4) },
        unsafe { weight_ptr.add(up4_off * 4) },
        unsafe { weight_ptr.add(up5_off * 4) },
        unsafe { weight_ptr.add(up6_off * 4) },
        unsafe { weight_ptr.add(up7_off * 4) },
    ];
    unsafe {
        linear_dot8_gate_up_interleaved_ptrs_unaligned(x_row_ptr, gate_ptrs, up_ptrs, in_features)
    }
}

unsafe fn linear_rows_f32(
    x_ptr: *const u8,
    weight_ptr: *const u8,
    out_ptr: *mut u8,
    outer: usize,
    in_features: usize,
    weight_row_start: usize,
    out_features: usize,
) {
    let x_total = outer.checked_mul(in_features);
    let weight_total = weight_row_start
        .checked_add(out_features)
        .and_then(|rows| rows.checked_mul(in_features));
    let out_total = outer.checked_mul(out_features);
    if let (Some(x_total), Some(weight_total), Some(out_total)) = (x_total, weight_total, out_total)
        && let (Some(x), Some(weight), Some(out)) = unsafe {
            (
                aligned_f32_slice(x_ptr, x_total),
                aligned_f32_slice(weight_ptr, weight_total),
                aligned_f32_slice_mut(out_ptr, out_total),
            )
        }
    {
        for batch in 0..outer {
            let x_off = batch * in_features;
            let out_off = batch * out_features;
            let mut out_idx = 0usize;
            while out_idx + 4 <= out_features {
                let w0_off = (weight_row_start + out_idx) * in_features;
                let w1_off = (weight_row_start + out_idx + 1) * in_features;
                let w2_off = (weight_row_start + out_idx + 2) * in_features;
                let w3_off = (weight_row_start + out_idx + 3) * in_features;
                let mut acc0 = 0.0f32;
                let mut acc1 = 0.0f32;
                let mut acc2 = 0.0f32;
                let mut acc3 = 0.0f32;
                for k in 0..in_features {
                    let xv = unsafe { *x.get_unchecked(x_off + k) };
                    acc0 += xv * unsafe { *weight.get_unchecked(w0_off + k) };
                    acc1 += xv * unsafe { *weight.get_unchecked(w1_off + k) };
                    acc2 += xv * unsafe { *weight.get_unchecked(w2_off + k) };
                    acc3 += xv * unsafe { *weight.get_unchecked(w3_off + k) };
                }
                unsafe {
                    *out.get_unchecked_mut(out_off + out_idx) = acc0;
                    *out.get_unchecked_mut(out_off + out_idx + 1) = acc1;
                    *out.get_unchecked_mut(out_off + out_idx + 2) = acc2;
                    *out.get_unchecked_mut(out_off + out_idx + 3) = acc3;
                }
                out_idx += 4;
            }
            while out_idx < out_features {
                let w_off = (weight_row_start + out_idx) * in_features;
                let mut acc = 0.0f32;
                for k in 0..in_features {
                    let xv = unsafe { *x.get_unchecked(x_off + k) };
                    acc += xv * unsafe { *weight.get_unchecked(w_off + k) };
                }
                unsafe { *out.get_unchecked_mut(out_off + out_idx) = acc };
                out_idx += 1;
            }
        }
        return;
    }

    #[cfg(any(
        target_arch = "aarch64",
        target_arch = "x86_64",
        all(target_arch = "wasm32", target_feature = "simd128")
    ))]
    {
        for batch in 0..outer {
            let x_off = batch * in_features;
            let out_off = batch * out_features;
            let x_row_ptr = unsafe { x_ptr.add(x_off * 4) };
            let mut out_idx = 0usize;
            while out_idx + 4 <= out_features {
                let w0_off = (weight_row_start + out_idx) * in_features;
                let w1_off = (weight_row_start + out_idx + 1) * in_features;
                let w2_off = (weight_row_start + out_idx + 2) * in_features;
                let w3_off = (weight_row_start + out_idx + 3) * in_features;
                unsafe {
                    linear_rows4_store_ptrs_unaligned(
                        x_row_ptr,
                        [
                            weight_ptr.add(w0_off * 4),
                            weight_ptr.add(w1_off * 4),
                            weight_ptr.add(w2_off * 4),
                            weight_ptr.add(w3_off * 4),
                        ],
                        [
                            out_ptr.add((out_off + out_idx) * 4),
                            out_ptr.add((out_off + out_idx + 1) * 4),
                            out_ptr.add((out_off + out_idx + 2) * 4),
                            out_ptr.add((out_off + out_idx + 3) * 4),
                        ],
                        in_features,
                    );
                }
                out_idx += 4;
            }
            while out_idx < out_features {
                let w_off = (weight_row_start + out_idx) * in_features;
                let sum =
                    unsafe { linear_dot4_unaligned(x_ptr, x_off, weight_ptr, w_off, in_features) };
                unsafe { (out_ptr.add((out_off + out_idx) * 4) as *mut f32).write_unaligned(sum) };
                out_idx += 1;
            }
        }
        return;
    }

    #[cfg(not(any(
        target_arch = "aarch64",
        target_arch = "x86_64",
        all(target_arch = "wasm32", target_feature = "simd128")
    )))]
    {
        for batch in 0..outer {
            let x_off = batch * in_features;
            let out_off = batch * out_features;
            let mut out_idx = 0usize;
            while out_idx + 4 <= out_features {
                let w0_off = (weight_row_start + out_idx) * in_features;
                let w1_off = (weight_row_start + out_idx + 1) * in_features;
                let w2_off = (weight_row_start + out_idx + 2) * in_features;
                let w3_off = (weight_row_start + out_idx + 3) * in_features;
                let mut acc0 = 0.0f32;
                let mut acc1 = 0.0f32;
                let mut acc2 = 0.0f32;
                let mut acc3 = 0.0f32;
                for k in 0..in_features {
                    let x = unsafe { (x_ptr.add((x_off + k) * 4) as *const f32).read_unaligned() };
                    let w0 = unsafe {
                        (weight_ptr.add((w0_off + k) * 4) as *const f32).read_unaligned()
                    };
                    let w1 = unsafe {
                        (weight_ptr.add((w1_off + k) * 4) as *const f32).read_unaligned()
                    };
                    let w2 = unsafe {
                        (weight_ptr.add((w2_off + k) * 4) as *const f32).read_unaligned()
                    };
                    let w3 = unsafe {
                        (weight_ptr.add((w3_off + k) * 4) as *const f32).read_unaligned()
                    };
                    acc0 += x * w0;
                    acc1 += x * w1;
                    acc2 += x * w2;
                    acc3 += x * w3;
                }
                unsafe {
                    (out_ptr.add((out_off + out_idx) * 4) as *mut f32).write_unaligned(acc0);
                    (out_ptr.add((out_off + out_idx + 1) * 4) as *mut f32).write_unaligned(acc1);
                    (out_ptr.add((out_off + out_idx + 2) * 4) as *mut f32).write_unaligned(acc2);
                    (out_ptr.add((out_off + out_idx + 3) * 4) as *mut f32).write_unaligned(acc3);
                }
                out_idx += 4;
            }
            while out_idx < out_features {
                let w_off = (weight_row_start + out_idx) * in_features;
                let mut acc = 0.0f32;
                for k in 0..in_features {
                    let x = unsafe { (x_ptr.add((x_off + k) * 4) as *const f32).read_unaligned() };
                    let w =
                        unsafe { (weight_ptr.add((w_off + k) * 4) as *const f32).read_unaligned() };
                    acc += x * w;
                }
                unsafe { (out_ptr.add((out_off + out_idx) * 4) as *mut f32).write_unaligned(acc) };
                out_idx += 1;
            }
        }
    }
}

#[cfg(any(
    target_arch = "aarch64",
    target_arch = "x86_64",
    all(target_arch = "wasm32", target_feature = "simd128")
))]
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
        let out_ptr = out_ptrs[part_idx];
        for batch in 0..outer {
            let x_off = batch * in_features;
            let out_off = batch * part_size;
            let x_row_ptr = unsafe { x_ptr.add(x_off * 4) };
            let mut out_idx = 0usize;
            while out_idx + 4 <= part_size {
                let row0_off = (prefix + out_idx) * in_features;
                let row1_off = (prefix + out_idx + 1) * in_features;
                let row2_off = (prefix + out_idx + 2) * in_features;
                let row3_off = (prefix + out_idx + 3) * in_features;
                unsafe {
                    linear_rows4_store_ptrs_unaligned(
                        x_row_ptr,
                        [
                            weight_ptr.add(row0_off * 4),
                            weight_ptr.add(row1_off * 4),
                            weight_ptr.add(row2_off * 4),
                            weight_ptr.add(row3_off * 4),
                        ],
                        [
                            out_ptr.add((out_off + out_idx) * 4),
                            out_ptr.add((out_off + out_idx + 1) * 4),
                            out_ptr.add((out_off + out_idx + 2) * 4),
                            out_ptr.add((out_off + out_idx + 3) * 4),
                        ],
                        in_features,
                    );
                }
                out_idx += 4;
            }
            while out_idx < part_size {
                let row_off = (prefix + out_idx) * in_features;
                let sum = unsafe {
                    linear_dot4_unaligned(x_ptr, x_off, weight_ptr, row_off, in_features)
                };
                unsafe { (out_ptr.add((out_off + out_idx) * 4) as *mut f32).write_unaligned(sum) };
                out_idx += 1;
            }
        }
        prefix += part_size;
    }
}

#[cfg(not(any(
    target_arch = "aarch64",
    target_arch = "x86_64",
    all(target_arch = "wasm32", target_feature = "simd128")
)))]
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
        unsafe {
            linear_rows_f32(
                x_ptr,
                weight_ptr,
                out_ptrs[part_idx],
                outer,
                in_features,
                prefix,
                part_size,
            );
        }
        prefix += part_size;
    }
}

unsafe fn linear_squared_relu_gate_interleaved_f32(
    x_ptr: *const u8,
    weight_ptr: *const u8,
    out_ptr: *mut u8,
    outer: usize,
    in_features: usize,
    hidden: usize,
) {
    let x_total = outer.checked_mul(in_features);
    let weight_total = hidden
        .checked_mul(2)
        .and_then(|rows| rows.checked_mul(in_features));
    let out_total = outer.checked_mul(hidden);
    if let (Some(x_total), Some(weight_total), Some(out_total)) = (x_total, weight_total, out_total)
        && let (Some(x), Some(weight), Some(out)) = unsafe {
            (
                aligned_f32_slice(x_ptr, x_total),
                aligned_f32_slice(weight_ptr, weight_total),
                aligned_f32_slice_mut(out_ptr, out_total),
            )
        }
    {
        for batch in 0..outer {
            let x_off = batch * in_features;
            let out_off = batch * hidden;
            let mut hidden_idx = 0usize;
            while hidden_idx + 4 <= hidden {
                let gate0_off = (2 * hidden_idx) * in_features;
                let up0_off = (2 * hidden_idx + 1) * in_features;
                let gate1_off = (2 * (hidden_idx + 1)) * in_features;
                let up1_off = (2 * (hidden_idx + 1) + 1) * in_features;
                let gate2_off = (2 * (hidden_idx + 2)) * in_features;
                let up2_off = (2 * (hidden_idx + 2) + 1) * in_features;
                let gate3_off = (2 * (hidden_idx + 3)) * in_features;
                let up3_off = (2 * (hidden_idx + 3) + 1) * in_features;
                let mut gate0 = 0.0f32;
                let mut up0 = 0.0f32;
                let mut gate1 = 0.0f32;
                let mut up1 = 0.0f32;
                let mut gate2 = 0.0f32;
                let mut up2 = 0.0f32;
                let mut gate3 = 0.0f32;
                let mut up3 = 0.0f32;
                for k in 0..in_features {
                    let xv = unsafe { *x.get_unchecked(x_off + k) };
                    gate0 += xv * unsafe { *weight.get_unchecked(gate0_off + k) };
                    up0 += xv * unsafe { *weight.get_unchecked(up0_off + k) };
                    gate1 += xv * unsafe { *weight.get_unchecked(gate1_off + k) };
                    up1 += xv * unsafe { *weight.get_unchecked(up1_off + k) };
                    gate2 += xv * unsafe { *weight.get_unchecked(gate2_off + k) };
                    up2 += xv * unsafe { *weight.get_unchecked(up2_off + k) };
                    gate3 += xv * unsafe { *weight.get_unchecked(gate3_off + k) };
                    up3 += xv * unsafe { *weight.get_unchecked(up3_off + k) };
                }
                unsafe {
                    let relu0 = gate0.max(0.0);
                    let relu1 = gate1.max(0.0);
                    let relu2 = gate2.max(0.0);
                    let relu3 = gate3.max(0.0);
                    *out.get_unchecked_mut(out_off + hidden_idx) = relu0 * relu0 * up0;
                    *out.get_unchecked_mut(out_off + hidden_idx + 1) = relu1 * relu1 * up1;
                    *out.get_unchecked_mut(out_off + hidden_idx + 2) = relu2 * relu2 * up2;
                    *out.get_unchecked_mut(out_off + hidden_idx + 3) = relu3 * relu3 * up3;
                }
                hidden_idx += 4;
            }
            while hidden_idx < hidden {
                let gate_off = (2 * hidden_idx) * in_features;
                let up_off = (2 * hidden_idx + 1) * in_features;
                let mut gate = 0.0f32;
                let mut up = 0.0f32;
                for k in 0..in_features {
                    let xv = unsafe { *x.get_unchecked(x_off + k) };
                    gate += xv * unsafe { *weight.get_unchecked(gate_off + k) };
                    up += xv * unsafe { *weight.get_unchecked(up_off + k) };
                }
                let relu = gate.max(0.0);
                unsafe { *out.get_unchecked_mut(out_off + hidden_idx) = relu * relu * up };
                hidden_idx += 1;
            }
        }
        return;
    }

    #[cfg(target_arch = "aarch64")]
    {
        let row_stride_bytes = in_features * 4;
        let pair_stride_bytes = row_stride_bytes * 2;
        for batch in 0..outer {
            let x_off = batch * in_features;
            let out_off = batch * hidden;
            let x_row_ptr = unsafe { x_ptr.add(x_off * 4) };
            let mut weight_group_ptr = weight_ptr;
            let mut out_group_ptr = unsafe { out_ptr.add(out_off * 4) };
            let mut hidden_idx = 0usize;
            while hidden_idx + 8 <= hidden {
                unsafe {
                    linear_gate_up8_store_group_unaligned(
                        x_row_ptr,
                        weight_group_ptr,
                        row_stride_bytes,
                        in_features,
                        out_group_ptr,
                    );
                }
                weight_group_ptr = unsafe { weight_group_ptr.add(pair_stride_bytes * 8) };
                out_group_ptr = unsafe { out_group_ptr.add(32) };
                hidden_idx += 8;
            }
            while hidden_idx + 4 <= hidden {
                unsafe {
                    linear_gate_up4_store_ptrs_unaligned(
                        x_row_ptr,
                        [
                            weight_group_ptr,
                            weight_group_ptr.add(pair_stride_bytes),
                            weight_group_ptr.add(pair_stride_bytes * 2),
                            weight_group_ptr.add(pair_stride_bytes * 3),
                        ],
                        [
                            weight_group_ptr.add(row_stride_bytes),
                            weight_group_ptr.add(pair_stride_bytes + row_stride_bytes),
                            weight_group_ptr.add(pair_stride_bytes * 2 + row_stride_bytes),
                            weight_group_ptr.add(pair_stride_bytes * 3 + row_stride_bytes),
                        ],
                        in_features,
                        out_group_ptr,
                    );
                }
                weight_group_ptr = unsafe { weight_group_ptr.add(pair_stride_bytes * 4) };
                out_group_ptr = unsafe { out_group_ptr.add(16) };
                hidden_idx += 4;
            }
            while hidden_idx < hidden {
                let gate_off = (2 * hidden_idx) * in_features;
                let up_off = (2 * hidden_idx + 1) * in_features;
                let gate_sum = unsafe {
                    linear_dot4_unaligned(x_ptr, x_off, weight_ptr, gate_off, in_features)
                };
                let up_sum =
                    unsafe { linear_dot4_unaligned(x_ptr, x_off, weight_ptr, up_off, in_features) };
                let relu = gate_sum.max(0.0);
                unsafe {
                    (out_ptr.add((out_off + hidden_idx) * 4) as *mut f32)
                        .write_unaligned(relu * relu * up_sum)
                };
                hidden_idx += 1;
            }
        }
        return;
    }

    #[cfg(any(
        target_arch = "x86_64",
        all(target_arch = "wasm32", target_feature = "simd128")
    ))]
    {
        let row_stride_bytes = in_features * 4;
        let pair_stride_bytes = row_stride_bytes * 2;
        for batch in 0..outer {
            let x_off = batch * in_features;
            let out_off = batch * hidden;
            let x_row_ptr = unsafe { x_ptr.add(x_off * 4) };
            let mut weight_group_ptr = weight_ptr;
            let mut out_group_ptr = unsafe { out_ptr.add(out_off * 4) };
            let mut hidden_idx = 0usize;
            while hidden_idx + 4 <= hidden {
                unsafe {
                    linear_gate_up4_store_ptrs_unaligned(
                        x_row_ptr,
                        [
                            weight_group_ptr,
                            weight_group_ptr.add(pair_stride_bytes),
                            weight_group_ptr.add(pair_stride_bytes * 2),
                            weight_group_ptr.add(pair_stride_bytes * 3),
                        ],
                        [
                            weight_group_ptr.add(row_stride_bytes),
                            weight_group_ptr.add(pair_stride_bytes + row_stride_bytes),
                            weight_group_ptr.add(pair_stride_bytes * 2 + row_stride_bytes),
                            weight_group_ptr.add(pair_stride_bytes * 3 + row_stride_bytes),
                        ],
                        in_features,
                        out_group_ptr,
                    );
                }
                weight_group_ptr = unsafe { weight_group_ptr.add(pair_stride_bytes * 4) };
                out_group_ptr = unsafe { out_group_ptr.add(16) };
                hidden_idx += 4;
            }
            while hidden_idx < hidden {
                let gate_off = (2 * hidden_idx) * in_features;
                let up_off = (2 * hidden_idx + 1) * in_features;
                let gate_sum = unsafe {
                    linear_dot4_unaligned(x_ptr, x_off, weight_ptr, gate_off, in_features)
                };
                let up_sum =
                    unsafe { linear_dot4_unaligned(x_ptr, x_off, weight_ptr, up_off, in_features) };
                let relu = gate_sum.max(0.0);
                unsafe {
                    (out_ptr.add((out_off + hidden_idx) * 4) as *mut f32)
                        .write_unaligned(relu * relu * up_sum)
                };
                hidden_idx += 1;
            }
        }
        return;
    }

    #[cfg(not(any(
        target_arch = "aarch64",
        target_arch = "x86_64",
        all(target_arch = "wasm32", target_feature = "simd128")
    )))]
    {
        for batch in 0..outer {
            let x_off = batch * in_features;
            let out_off = batch * hidden;
            let mut hidden_idx = 0usize;
            while hidden_idx + 4 <= hidden {
                let gate0_off = (2 * hidden_idx) * in_features;
                let up0_off = (2 * hidden_idx + 1) * in_features;
                let gate1_off = (2 * (hidden_idx + 1)) * in_features;
                let up1_off = (2 * (hidden_idx + 1) + 1) * in_features;
                let gate2_off = (2 * (hidden_idx + 2)) * in_features;
                let up2_off = (2 * (hidden_idx + 2) + 1) * in_features;
                let gate3_off = (2 * (hidden_idx + 3)) * in_features;
                let up3_off = (2 * (hidden_idx + 3) + 1) * in_features;
                let mut gate0 = 0.0f32;
                let mut up0 = 0.0f32;
                let mut gate1 = 0.0f32;
                let mut up1 = 0.0f32;
                let mut gate2 = 0.0f32;
                let mut up2 = 0.0f32;
                let mut gate3 = 0.0f32;
                let mut up3 = 0.0f32;
                for k in 0..in_features {
                    let x = unsafe { (x_ptr.add((x_off + k) * 4) as *const f32).read_unaligned() };
                    let gate0_w = unsafe {
                        (weight_ptr.add((gate0_off + k) * 4) as *const f32).read_unaligned()
                    };
                    let up0_w = unsafe {
                        (weight_ptr.add((up0_off + k) * 4) as *const f32).read_unaligned()
                    };
                    let gate1_w = unsafe {
                        (weight_ptr.add((gate1_off + k) * 4) as *const f32).read_unaligned()
                    };
                    let up1_w = unsafe {
                        (weight_ptr.add((up1_off + k) * 4) as *const f32).read_unaligned()
                    };
                    let gate2_w = unsafe {
                        (weight_ptr.add((gate2_off + k) * 4) as *const f32).read_unaligned()
                    };
                    let up2_w = unsafe {
                        (weight_ptr.add((up2_off + k) * 4) as *const f32).read_unaligned()
                    };
                    let gate3_w = unsafe {
                        (weight_ptr.add((gate3_off + k) * 4) as *const f32).read_unaligned()
                    };
                    let up3_w = unsafe {
                        (weight_ptr.add((up3_off + k) * 4) as *const f32).read_unaligned()
                    };
                    gate0 += x * gate0_w;
                    up0 += x * up0_w;
                    gate1 += x * gate1_w;
                    up1 += x * up1_w;
                    gate2 += x * gate2_w;
                    up2 += x * up2_w;
                    gate3 += x * gate3_w;
                    up3 += x * up3_w;
                }
                unsafe {
                    let relu0 = gate0.max(0.0);
                    let relu1 = gate1.max(0.0);
                    let relu2 = gate2.max(0.0);
                    let relu3 = gate3.max(0.0);
                    (out_ptr.add((out_off + hidden_idx) * 4) as *mut f32)
                        .write_unaligned(relu0 * relu0 * up0);
                    (out_ptr.add((out_off + hidden_idx + 1) * 4) as *mut f32)
                        .write_unaligned(relu1 * relu1 * up1);
                    (out_ptr.add((out_off + hidden_idx + 2) * 4) as *mut f32)
                        .write_unaligned(relu2 * relu2 * up2);
                    (out_ptr.add((out_off + hidden_idx + 3) * 4) as *mut f32)
                        .write_unaligned(relu3 * relu3 * up3);
                }
                hidden_idx += 4;
            }
            while hidden_idx < hidden {
                let gate_off = (2 * hidden_idx) * in_features;
                let up_off = (2 * hidden_idx + 1) * in_features;
                let mut gate = 0.0f32;
                let mut up = 0.0f32;
                for k in 0..in_features {
                    let x = unsafe { (x_ptr.add((x_off + k) * 4) as *const f32).read_unaligned() };
                    let gate_w = unsafe {
                        (weight_ptr.add((gate_off + k) * 4) as *const f32).read_unaligned()
                    };
                    let up_w = unsafe {
                        (weight_ptr.add((up_off + k) * 4) as *const f32).read_unaligned()
                    };
                    gate += x * gate_w;
                    up += x * up_w;
                }
                let relu = gate.max(0.0);
                unsafe {
                    (out_ptr.add((out_off + hidden_idx) * 4) as *mut f32)
                        .write_unaligned(relu * relu * up);
                }
                hidden_idx += 1;
            }
        }
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
                        (a_ptr.add((a_off + i * a_cols + k) * 4) as *const f32).read_unaligned()
                    };
                    let b = unsafe {
                        (b_ptr.add((b_off + k * b_cols + j) * 4) as *const f32).read_unaligned()
                    };
                    acc += a * b;
                }
                unsafe {
                    (out_ptr.add((out_off + i * b_cols + j) * 4) as *mut f32).write_unaligned(acc);
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
                                (cos_ptr.add((freq_base + i) * 4) as *const f32).read_unaligned(),
                                (sin_ptr.add((freq_base + i) * 4) as *const f32).read_unaligned(),
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

unsafe fn softmax_last_axis_f32(x_ptr: *const u8, out_ptr: *mut u8, outer: usize, axis_len: usize) {
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

unsafe fn squared_relu_gate_interleaved_f32(
    x_ptr: *const u8,
    out_ptr: *mut u8,
    outer: usize,
    axis_len: usize,
) {
    let hidden = axis_len / 2;
    for row in 0..outer {
        let in_base = row * axis_len;
        let out_base = row * hidden;
        for i in 0..hidden {
            let gate = unsafe { (x_ptr.add((in_base + 2 * i) * 4) as *const f32).read_unaligned() };
            let up =
                unsafe { (x_ptr.add((in_base + 2 * i + 1) * 4) as *const f32).read_unaligned() };
            let relu = gate.max(0.0);
            unsafe {
                (out_ptr.add((out_base + i) * 4) as *mut f32).write_unaligned(relu * relu * up);
            }
        }
    }
}

#[derive(Copy, Clone)]
struct BufferRuntimeView {
    class_bits: u64,
    data_bits: u64,
    data_view: ByteView,
    element_type_bits: u64,
    format_bits: u64,
    format: ScalarFormat,
    size: usize,
}

#[derive(Copy, Clone)]
struct TensorRuntimeView {
    class_bits: u64,
    buffer_bits: u64,
    buffer: BufferRuntimeView,
    shape_bits: u64,
    dtype_bits: u64,
}

unsafe fn buffer_runtime_view(
    _py: &crate::PyToken<'_>,
    buffer_bits: u64,
    role: &str,
) -> Result<BufferRuntimeView, u64> {
    let Some(buffer_ptr) = obj_from_bits(buffer_bits).as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{role} must be a Buffer instance"),
        ));
    };
    let class_bits = unsafe { crate::object_class_bits(buffer_ptr) };
    if obj_from_bits(class_bits).is_none() {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{role} must be a Buffer instance"),
        ));
    }
    let data_bits = unsafe { object_attr_bits(_py, buffer_bits, b"_data", "_data") }?;
    let element_type_bits =
        unsafe { object_attr_bits(_py, buffer_bits, b"_element_type", "_element_type") }?;
    let size_bits = unsafe { object_attr_bits(_py, buffer_bits, b"_size", "_size") }?;
    let format_bits =
        unsafe { object_attr_bits(_py, buffer_bits, b"_format_char", "_format_char") }?;
    let size = parse_usize_arg(_py, size_bits, "buffer._size")?;
    let format = parse_format(_py, format_bits, "buffer._format_char")?;
    let data_view = bytes_like_view(_py, data_bits, "buffer._data")?;
    Ok(BufferRuntimeView {
        class_bits,
        data_bits,
        data_view,
        element_type_bits,
        format_bits,
        format,
        size,
    })
}

unsafe fn tensor_runtime_view(
    _py: &crate::PyToken<'_>,
    tensor_bits: u64,
    role: &str,
) -> Result<(TensorRuntimeView, Vec<usize>), u64> {
    let Some(tensor_ptr) = obj_from_bits(tensor_bits).as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{role} must be a Tensor instance"),
        ));
    };
    let class_bits = unsafe { crate::object_class_bits(tensor_ptr) };
    if obj_from_bits(class_bits).is_none() {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{role} must be a Tensor instance"),
        ));
    }
    let buffer_bits = unsafe { object_attr_bits(_py, tensor_bits, b"_buf", "_buf") }?;
    let shape_bits = unsafe { object_attr_bits(_py, tensor_bits, b"_shape", "_shape") }?;
    let dtype_bits = unsafe { object_attr_bits(_py, tensor_bits, b"_dtype", "_dtype") }?;
    let shape = parse_shape(_py, shape_bits, "tensor._shape")?;
    let buffer = unsafe { buffer_runtime_view(_py, buffer_bits, "tensor._buf") }?;
    Ok((
        TensorRuntimeView {
            class_bits,
            buffer_bits,
            buffer,
            shape_bits,
            dtype_bits,
        },
        shape,
    ))
}

fn alloc_string_bits(_py: &crate::PyToken<'_>, value: &[u8]) -> Result<u64, u64> {
    let ptr = crate::alloc_string(_py, value);
    if ptr.is_null() {
        Err(MoltObject::none().bits())
    } else {
        Ok(MoltObject::from_ptr(ptr).bits())
    }
}

fn alloc_tuple_bits_from_usize(
    _py: &crate::PyToken<'_>,
    dims: &[usize],
) -> Result<u64, u64> {
    let bits: Vec<u64> = dims
        .iter()
        .copied()
        .map(|dim| MoltObject::from_int(dim as i64).bits())
        .collect();
    let ptr = alloc_tuple(_py, bits.as_slice());
    if ptr.is_null() {
        Err(MoltObject::none().bits())
    } else {
        Ok(MoltObject::from_ptr(ptr).bits())
    }
}

fn normalize_sequence_arg_bits(
    _py: &crate::PyToken<'_>,
    bits: u64,
    role: &str,
    allow_scalar_int: bool,
) -> Result<Vec<u64>, u64> {
    let obj = obj_from_bits(bits);
    let mut elems = if let Some(ptr) = obj.as_ptr() {
        match unsafe { object_type_id(ptr) } {
            TYPE_ID_TUPLE | TYPE_ID_LIST => unsafe { seq_vec_ref(ptr) }.to_vec(),
            _ => {
                if allow_scalar_int && to_i64(obj).is_some() {
                    vec![bits]
                } else {
                    return Err(raise_exception::<_>(
                        _py,
                        "TypeError",
                        &format!("{role} must be a tuple or list of ints"),
                    ));
                }
            }
        }
    } else if allow_scalar_int && to_i64(obj).is_some() {
        vec![bits]
    } else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{role} must be a tuple or list of ints"),
        ));
    };
    if elems.len() == 1 {
        let inner = obj_from_bits(elems[0]);
        if let Some(inner_ptr) = inner.as_ptr() {
            let ty = unsafe { object_type_id(inner_ptr) };
            if ty == TYPE_ID_TUPLE || ty == TYPE_ID_LIST {
                elems = unsafe { seq_vec_ref(inner_ptr) }.to_vec();
            }
        }
    }
    Ok(elems)
}

fn parse_i64_sequence_arg(
    _py: &crate::PyToken<'_>,
    bits: u64,
    role: &str,
    allow_scalar_int: bool,
) -> Result<Vec<i64>, u64> {
    let elems = normalize_sequence_arg_bits(_py, bits, role, allow_scalar_int)?;
    let mut out = Vec::with_capacity(elems.len());
    for elem_bits in elems {
        let Some(value) = to_i64(obj_from_bits(elem_bits)) else {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                &format!("{role} must contain integers"),
            ));
        };
        out.push(value);
    }
    Ok(out)
}

fn normalize_permute_dims(
    _py: &crate::PyToken<'_>,
    dims_bits: u64,
    ndim: usize,
) -> Result<Vec<usize>, u64> {
    let raw_dims = parse_i64_sequence_arg(_py, dims_bits, "dims", false)?;
    if raw_dims.len() != ndim {
        return Err(raise_exception::<_>(
            _py,
            "ValueError",
            "permute dims must match tensor rank",
        ));
    }
    let mut normalized = Vec::with_capacity(raw_dims.len());
    for raw_dim in raw_dims {
        let mut dim = raw_dim;
        if dim < 0 {
            dim += ndim as i64;
        }
        if dim < 0 || dim >= ndim as i64 {
            return Err(raise_exception::<_>(
                _py,
                "ValueError",
                &format!("permute dim {raw_dim} out of range for ndim={ndim}"),
            ));
        }
        normalized.push(dim as usize);
    }
    validate_permutation(_py, normalized.as_slice(), ndim)?;
    Ok(normalized)
}

fn normalize_reshape_dims(_py: &crate::PyToken<'_>, shape_bits: u64) -> Result<Vec<i64>, u64> {
    parse_i64_sequence_arg(_py, shape_bits, "shape", true)
}

unsafe fn module_global_bits(
    _py: &crate::PyToken<'_>,
    module_name: &[u8],
    attr_name: &[u8],
    attr_label: &str,
) -> Result<u64, u64> {
    let module_name_bits = alloc_string_bits(_py, module_name)?;
    let mut module_bits = crate::builtins::modules::molt_module_cache_get(module_name_bits);
    if obj_from_bits(module_bits).is_none() {
        module_bits = crate::builtins::modules::molt_module_import(module_name_bits);
    }
    crate::dec_ref_bits(_py, module_name_bits);
    if crate::exception_pending(_py) && obj_from_bits(module_bits).as_ptr().is_some() {
        let _ = crate::molt_exception_clear();
    }
    if crate::exception_pending(_py) {
        return Err(module_bits);
    }
    let attr_bits = crate::attr_name_bits_from_bytes(_py, attr_name)
        .ok_or_else(|| MoltObject::none().bits())?;
    let missing = crate::builtins::methods::missing_bits(_py);
    let value_bits = crate::molt_getattr_builtin(module_bits, attr_bits, missing);
    crate::dec_ref_bits(_py, attr_bits);
    crate::dec_ref_bits(_py, module_bits);
    if crate::exception_pending(_py) && !crate::builtins::methods::is_missing_bits(_py, value_bits)
    {
        let _ = crate::molt_exception_clear();
    }
    if crate::exception_pending(_py) {
        return Err(value_bits);
    }
    if crate::builtins::methods::is_missing_bits(_py, value_bits) {
        return Err(raise_exception::<_>(
            _py,
            "AttributeError",
            &format!(
                "module {:?} has no attribute {:?}",
                String::from_utf8_lossy(module_name),
                attr_label
            ),
        ));
    }
    Ok(value_bits)
}

unsafe fn ensure_tensor_object_bits(_py: &crate::PyToken<'_>, value_bits: u64) -> Result<u64, u64> {
    let tensor_class_bits =
        unsafe { module_global_bits(_py, b"molt.gpu.tensor", b"Tensor", "Tensor") }?;
    let is_tensor_bits = crate::molt_isinstance(value_bits, tensor_class_bits);
    if crate::exception_pending(_py) {
        crate::dec_ref_bits(_py, tensor_class_bits);
        return Err(is_tensor_bits);
    }
    let is_tensor = crate::is_truthy(_py, obj_from_bits(is_tensor_bits));
    crate::dec_ref_bits(_py, is_tensor_bits);
    if is_tensor {
        crate::dec_ref_bits(_py, tensor_class_bits);
        return Ok(value_bits);
    }
    let tensor_bits = unsafe { crate::call::dispatch::call_callable1(_py, tensor_class_bits, value_bits) };
    crate::dec_ref_bits(_py, tensor_class_bits);
    if crate::exception_pending(_py) {
        return Err(tensor_bits);
    }
    Ok(tensor_bits)
}

unsafe fn promoted_result_format_bits(
    _py: &crate::PyToken<'_>,
    x: &TensorRuntimeView,
    weight: &TensorRuntimeView,
) -> Result<(u64, ScalarFormat, bool, u64), u64> {
    let float_bits = crate::builtins::classes::builtin_classes(_py).float;
    if x.dtype_bits == float_bits && weight.dtype_bits == float_bits {
        if x.buffer.element_type_bits == float_bits
            && weight.buffer.element_type_bits == float_bits
            && x.buffer.format == ScalarFormat::F32
            && weight.buffer.format == ScalarFormat::F32
        {
            return Ok((alloc_string_bits(_py, b"f")?, ScalarFormat::F32, true, x.dtype_bits));
        }
        return Ok((alloc_string_bits(_py, b"d")?, ScalarFormat::F64, true, x.dtype_bits));
    }
    Ok((x.buffer.format_bits, x.buffer.format, false, x.dtype_bits))
}

unsafe fn build_tensor_from_data_bits(
    _py: &crate::PyToken<'_>,
    tensor_class_bits: u64,
    buffer_class_bits: u64,
    data_bits: u64,
    element_type_bits: u64,
    size: usize,
    format_bits: u64,
    itemsize: usize,
    shape_bits: u64,
    dtype_bits: u64,
) -> Result<u64, u64> {
    let buffer_bits = unsafe {
        build_buffer_instance(
            _py,
            buffer_class_bits,
            data_bits,
            element_type_bits,
            size,
            format_bits,
            itemsize,
        )
    }?;
    let tensor_bits =
        unsafe { build_tensor_instance(_py, tensor_class_bits, buffer_bits, shape_bits, dtype_bits) };
    crate::dec_ref_bits(_py, buffer_bits);
    tensor_bits
}

#[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
#[allow(non_snake_case)]
pub extern "C" fn molt_gpu_tensor__tensor_linear(x_bits: u64, weight_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let (x, x_shape) = match unsafe { tensor_runtime_view(_py, x_bits, "x") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let (weight, weight_shape) = match unsafe { tensor_runtime_view(_py, weight_bits, "weight") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if weight_shape.len() != 2 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                &format!("linear weight must be 2D, got {:?}", weight_shape),
            );
        }
        if x_shape.is_empty() {
            return raise_exception::<_>(_py, "ValueError", "linear input must be at least 1D");
        }
        let in_features = *x_shape.last().unwrap_or(&0);
        let out_features = weight_shape[0];
        let weight_in = weight_shape[1];
        if in_features != weight_in {
            return raise_exception::<_>(
                _py,
                "ValueError",
                &format!("Linear shape mismatch: {:?} with weight {:?}", x_shape, weight_shape),
            );
        }
        let outer = if x_shape.len() > 1 {
            product(&x_shape[..x_shape.len() - 1])
        } else {
            1
        };
        let out_shape = if x_shape.len() > 1 {
            let mut dims = x_shape[..x_shape.len() - 1].to_vec();
            dims.push(out_features);
            dims
        } else {
            vec![out_features]
        };
        let (out_format_bits, out_format, owns_out_format, result_dtype_bits) =
            match unsafe { promoted_result_format_bits(_py, &x, &weight) } {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let out_data_bits = molt_gpu_linear_contiguous(
            x.buffer.data_bits,
            x.buffer.format_bits,
            weight.buffer.data_bits,
            weight.buffer.format_bits,
            MoltObject::from_int(outer as i64).bits(),
            MoltObject::from_int(in_features as i64).bits(),
            MoltObject::from_int(out_features as i64).bits(),
            out_format_bits,
        );
        if crate::exception_pending(_py) {
            if owns_out_format {
                crate::dec_ref_bits(_py, out_format_bits);
            }
            return out_data_bits;
        }
        let out_shape_bits = match alloc_tuple_bits_from_usize(_py, out_shape.as_slice()) {
            Ok(bits) => bits,
            Err(bits) => {
                if owns_out_format {
                    crate::dec_ref_bits(_py, out_format_bits);
                }
                return bits;
            }
        };
        let tensor_bits = match unsafe {
            build_tensor_from_data_bits(
                _py,
                x.class_bits,
                x.buffer.class_bits,
                out_data_bits,
                result_dtype_bits,
                outer * out_features,
                out_format_bits,
                out_format.itemsize(),
                out_shape_bits,
                result_dtype_bits,
            )
        } {
            Ok(bits) => bits,
            Err(bits) => bits,
        };
        crate::dec_ref_bits(_py, out_data_bits);
        crate::dec_ref_bits(_py, out_shape_bits);
        if owns_out_format {
            crate::dec_ref_bits(_py, out_format_bits);
        }
        tensor_bits
    })
}

#[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
#[allow(non_snake_case)]
pub extern "C" fn molt_gpu_tensor__tensor_linear_split_last_dim(
    x_bits: u64,
    weight_bits: u64,
    split_sizes_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let (x, x_shape) = match unsafe { tensor_runtime_view(_py, x_bits, "x") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let (weight, weight_shape) = match unsafe { tensor_runtime_view(_py, weight_bits, "weight") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if weight_shape.len() != 2 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                &format!("linear weight must be 2D, got {:?}", weight_shape),
            );
        }
        if x_shape.is_empty() {
            return raise_exception::<_>(_py, "ValueError", "linear input must be at least 1D");
        }
        let split_sizes = match parse_shape(_py, split_sizes_bits, "split_sizes") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let in_features = *x_shape.last().unwrap_or(&0);
        let out_features = weight_shape[0];
        let weight_in = weight_shape[1];
        if in_features != weight_in {
            return raise_exception::<_>(
                _py,
                "ValueError",
                &format!("Linear shape mismatch: {:?} with weight {:?}", x_shape, weight_shape),
            );
        }
        if split_sizes.iter().copied().sum::<usize>() != out_features {
            return raise_exception::<_>(
                _py,
                "ValueError",
                &format!(
                    "split sizes {:?} do not match projected dimension {}",
                    split_sizes, out_features
                ),
            );
        }
        let outer = if x_shape.len() > 1 {
            product(&x_shape[..x_shape.len() - 1])
        } else {
            1
        };
        let prefix_shape = if x_shape.len() > 1 {
            x_shape[..x_shape.len() - 1].to_vec()
        } else {
            Vec::new()
        };
        let (out_format_bits, out_format, owns_out_format, result_dtype_bits) =
            match unsafe { promoted_result_format_bits(_py, &x, &weight) } {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let out_parts_bits = molt_gpu_linear_split_last_dim_contiguous(
            x.buffer.data_bits,
            x.buffer.format_bits,
            weight.buffer.data_bits,
            weight.buffer.format_bits,
            MoltObject::from_int(outer as i64).bits(),
            MoltObject::from_int(in_features as i64).bits(),
            split_sizes_bits,
            out_format_bits,
        );
        if crate::exception_pending(_py) {
            if owns_out_format {
                crate::dec_ref_bits(_py, out_format_bits);
            }
            return out_parts_bits;
        }
        let Some(out_parts_ptr) = obj_from_bits(out_parts_bits).as_ptr() else {
            if owns_out_format {
                crate::dec_ref_bits(_py, out_format_bits);
            }
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "linear split helper did not return a tuple",
            );
        };
        let part_data_bits = unsafe { seq_vec_ref(out_parts_ptr) };
        if part_data_bits.len() != split_sizes.len() {
            if owns_out_format {
                crate::dec_ref_bits(_py, out_format_bits);
            }
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "intrinsic returned wrong split count",
            );
        }
        let mut tensors = Vec::with_capacity(split_sizes.len());
        for (idx, &part_size) in split_sizes.iter().enumerate() {
            let mut dims = prefix_shape.clone();
            dims.push(part_size);
            let shape_bits = match alloc_tuple_bits_from_usize(_py, dims.as_slice()) {
                Ok(bits) => bits,
                Err(bits) => {
                    if owns_out_format {
                        crate::dec_ref_bits(_py, out_format_bits);
                    }
                    return bits;
                }
            };
            let tensor_bits = match unsafe {
                build_tensor_from_data_bits(
                    _py,
                    x.class_bits,
                    x.buffer.class_bits,
                    part_data_bits[idx],
                    result_dtype_bits,
                    outer * part_size,
                    out_format_bits,
                    out_format.itemsize(),
                    shape_bits,
                    result_dtype_bits,
                )
            } {
                Ok(bits) => bits,
                Err(bits) => {
                    crate::dec_ref_bits(_py, shape_bits);
                    if owns_out_format {
                        crate::dec_ref_bits(_py, out_format_bits);
                    }
                    return bits;
                }
            };
            crate::dec_ref_bits(_py, shape_bits);
            tensors.push(tensor_bits);
        }
        let tuple_ptr = alloc_tuple(_py, tensors.as_slice());
        if owns_out_format {
            crate::dec_ref_bits(_py, out_format_bits);
        }
        if tuple_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
#[allow(non_snake_case)]
pub extern "C" fn molt_gpu_tensor__tensor_linear_squared_relu_gate_interleaved(
    x_bits: u64,
    weight_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let (x, x_shape) = match unsafe { tensor_runtime_view(_py, x_bits, "x") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let (weight, weight_shape) = match unsafe { tensor_runtime_view(_py, weight_bits, "weight") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if weight_shape.len() != 2 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                &format!("linear weight must be 2D, got {:?}", weight_shape),
            );
        }
        if x_shape.is_empty() {
            return raise_exception::<_>(_py, "ValueError", "linear input must be at least 1D");
        }
        let in_features = *x_shape.last().unwrap_or(&0);
        let out_features = weight_shape[0];
        let weight_in = weight_shape[1];
        if in_features != weight_in {
            return raise_exception::<_>(
                _py,
                "ValueError",
                &format!("Linear shape mismatch: {:?} with weight {:?}", x_shape, weight_shape),
            );
        }
        if out_features % 2 != 0 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                &format!(
                    "interleaved gate weight output dimension must be even, got {}",
                    out_features
                ),
            );
        }
        let outer = if x_shape.len() > 1 {
            product(&x_shape[..x_shape.len() - 1])
        } else {
            1
        };
        let hidden = out_features / 2;
        let out_shape = if x_shape.len() > 1 {
            let mut dims = x_shape[..x_shape.len() - 1].to_vec();
            dims.push(hidden);
            dims
        } else {
            vec![hidden]
        };
        let (out_format_bits, out_format, owns_out_format, result_dtype_bits) =
            match unsafe { promoted_result_format_bits(_py, &x, &weight) } {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let out_data_bits = molt_gpu_linear_squared_relu_gate_interleaved_contiguous(
            x.buffer.data_bits,
            x.buffer.format_bits,
            weight.buffer.data_bits,
            weight.buffer.format_bits,
            MoltObject::from_int(outer as i64).bits(),
            MoltObject::from_int(in_features as i64).bits(),
            out_format_bits,
        );
        if crate::exception_pending(_py) {
            if owns_out_format {
                crate::dec_ref_bits(_py, out_format_bits);
            }
            return out_data_bits;
        }
        let out_shape_bits = match alloc_tuple_bits_from_usize(_py, out_shape.as_slice()) {
            Ok(bits) => bits,
            Err(bits) => {
                if owns_out_format {
                    crate::dec_ref_bits(_py, out_format_bits);
                }
                return bits;
            }
        };
        let tensor_bits = match unsafe {
            build_tensor_from_data_bits(
                _py,
                x.class_bits,
                x.buffer.class_bits,
                out_data_bits,
                result_dtype_bits,
                outer * hidden,
                out_format_bits,
                out_format.itemsize(),
                out_shape_bits,
                result_dtype_bits,
            )
        } {
            Ok(bits) => bits,
            Err(bits) => bits,
        };
        crate::dec_ref_bits(_py, out_data_bits);
        crate::dec_ref_bits(_py, out_shape_bits);
        if owns_out_format {
            crate::dec_ref_bits(_py, out_format_bits);
        }
        tensor_bits
    })
}

#[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
#[allow(non_snake_case)]
pub extern "C" fn molt_gpu_tensor__tensor_permute_dims(x_bits: u64, dims_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let (x, x_shape) = match unsafe { tensor_runtime_view(_py, x_bits, "x") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let normalized_dims = match normalize_permute_dims(_py, dims_bits, x_shape.len()) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if x_shape.len() <= 1 {
            return match unsafe {
                build_tensor_instance(_py, x.class_bits, x.buffer_bits, x.shape_bits, x.dtype_bits)
            } {
                Ok(bits) => bits,
                Err(bits) => bits,
            };
        }
        let normalized_dims_bits = match alloc_tuple_bits_from_usize(_py, normalized_dims.as_slice()) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let out_data_bits = molt_gpu_permute_contiguous(
            x.buffer.data_bits,
            x.buffer.format_bits,
            x.shape_bits,
            normalized_dims_bits,
            x.buffer.format_bits,
        );
        crate::dec_ref_bits(_py, normalized_dims_bits);
        if crate::exception_pending(_py) {
            return out_data_bits;
        }
        let out_shape: Vec<usize> = normalized_dims.iter().map(|&dim| x_shape[dim]).collect();
        let out_shape_bits = match alloc_tuple_bits_from_usize(_py, out_shape.as_slice()) {
            Ok(bits) => bits,
            Err(bits) => {
                crate::dec_ref_bits(_py, out_data_bits);
                return bits;
            }
        };
        let tensor_bits = match unsafe {
            build_tensor_from_data_bits(
                _py,
                x.class_bits,
                x.buffer.class_bits,
                out_data_bits,
                x.buffer.element_type_bits,
                if x_shape.is_empty() { 1 } else { product(&x_shape) },
                x.buffer.format_bits,
                x.buffer.format.itemsize(),
                out_shape_bits,
                x.dtype_bits,
            )
        } {
            Ok(bits) => bits,
            Err(bits) => bits,
        };
        crate::dec_ref_bits(_py, out_data_bits);
        crate::dec_ref_bits(_py, out_shape_bits);
        tensor_bits
    })
}

#[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
#[allow(non_snake_case)]
pub extern "C" fn molt_gpu_tensor__tensor_softmax_last_axis(x_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let (x, x_shape) = match unsafe { tensor_runtime_view(_py, x_bits, "x") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let shape_bits = match alloc_tuple_bits_from_usize(_py, x_shape.as_slice()) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let out_data_bits = molt_gpu_softmax_last_axis_contiguous(
            x.buffer.data_bits,
            x.buffer.format_bits,
            shape_bits,
            x.buffer.format_bits,
        );
        if crate::exception_pending(_py) {
            crate::dec_ref_bits(_py, shape_bits);
            return out_data_bits;
        }
        let tensor_bits = match unsafe {
            build_tensor_from_data_bits(
                _py,
                x.class_bits,
                x.buffer.class_bits,
                out_data_bits,
                x.buffer.element_type_bits,
                x.buffer.size,
                x.buffer.format_bits,
                x.buffer.format.itemsize(),
                shape_bits,
                x.dtype_bits,
            )
        } {
            Ok(bits) => bits,
            Err(bits) => bits,
        };
        crate::dec_ref_bits(_py, out_data_bits);
        crate::dec_ref_bits(_py, shape_bits);
        tensor_bits
    })
}

#[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
#[allow(non_snake_case)]
pub extern "C" fn molt_gpu_tensor__tensor_reshape_view(x_bits: u64, shape_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let (x, x_shape) = match unsafe { tensor_runtime_view(_py, x_bits, "x") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let total_size = if x_shape.is_empty() { 1 } else { product(&x_shape) };
        let mut dims = match normalize_reshape_dims(_py, shape_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut neg_idx = None;
        let mut known = 1i64;
        for (idx, dim) in dims.iter().copied().enumerate() {
            if dim == -1 {
                if neg_idx.is_some() {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "Only one dimension can be -1",
                    );
                }
                neg_idx = Some(idx);
            } else {
                known = known.saturating_mul(dim);
            }
        }
        if let Some(idx) = neg_idx {
            if known == 0 {
                return raise_exception::<_>(_py, "ZeroDivisionError", "integer division or modulo by zero");
            }
            dims[idx] = (total_size as i64) / known;
        }
        let mut final_shape = Vec::with_capacity(dims.len());
        for dim in dims.iter().copied() {
            let value = usize::try_from(dim).map_err(|_| {
                raise_exception::<u64>(
                    _py,
                    "ValueError",
                    &format!("Cannot reshape tensor of size {} into shape {:?}", total_size, dims),
                )
            });
            match value {
                Ok(value) => final_shape.push(value),
                Err(bits) => return bits,
            }
        }
        if product(final_shape.as_slice()) != total_size {
            return raise_exception::<_>(
                _py,
                "ValueError",
                &format!(
                    "Cannot reshape tensor of size {} into shape {:?}",
                    total_size, final_shape
                ),
            );
        }
        let final_shape_bits = match alloc_tuple_bits_from_usize(_py, final_shape.as_slice()) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let tensor_bits =
            match unsafe { build_tensor_instance(_py, x.class_bits, x.buffer_bits, final_shape_bits, x.dtype_bits) } {
                Ok(bits) => bits,
                Err(bits) => bits,
            };
        crate::dec_ref_bits(_py, final_shape_bits);
        tensor_bits
    })
}

#[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
#[allow(non_snake_case)]
pub extern "C" fn molt_gpu_tensor__tensor_data_list(x_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let (x, x_shape) = match unsafe { tensor_runtime_view(_py, x_bits, "x") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let size = if x_shape.is_empty() { 1 } else { product(&x_shape) };
        molt_gpu_buffer_to_list(x.buffer_bits, MoltObject::from_int(size as i64).bits())
    })
}

#[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
#[allow(non_snake_case)]
pub extern "C" fn molt_gpu_tensor__tensor_take_rows(
    x_bits: u64,
    indices_bits: u64,
    allow_negative_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let (x, x_shape) = match unsafe { tensor_runtime_view(_py, x_bits, "x") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if x_shape.is_empty() {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "take_rows requires a tensor with at least 1 dimension",
            );
        }
        let indices_tensor_bits = match unsafe { ensure_tensor_object_bits(_py, indices_bits) } {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let (indices, indices_shape) =
            match unsafe { tensor_runtime_view(_py, indices_tensor_bits, "indices") } {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let row_count = if indices_shape.is_empty() {
            1
        } else {
            product(&indices_shape)
        };
        let rows_list_bits = molt_gpu_buffer_to_list(
            indices.buffer_bits,
            MoltObject::from_int(row_count as i64).bits(),
        );
        if crate::exception_pending(_py) {
            return rows_list_bits;
        }
        let Some(rows_list_ptr) = obj_from_bits(rows_list_bits).as_ptr() else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                "indices tensor did not materialize to a list",
            );
        };
        let row_shape = &x_shape[1..];
        let row_size = if row_shape.is_empty() { 1 } else { product(row_shape) };
        let width = row_size * x.buffer.format.itemsize();
        let expected_bytes = x.buffer.size * x.buffer.format.itemsize();
        if x.buffer.data_view.len < expected_bytes {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "x_data buffer is too small",
            );
        }
        let allow_negative = crate::is_truthy(_py, obj_from_bits(allow_negative_bits));
        let rows = unsafe { seq_vec_ref(rows_list_ptr) };
        let src = unsafe { std::slice::from_raw_parts(x.buffer.data_view.ptr, expected_bytes) };
        let mut out = vec![0u8; rows.len() * width];
        for (out_row, &raw_idx_bits) in rows.iter().enumerate() {
            let raw_idx_obj = obj_from_bits(raw_idx_bits);
            let idx = if let Some(value) = to_i64(raw_idx_obj) {
                value
            } else if let Some(value) = to_f64(raw_idx_obj) {
                let idx = value as i64;
                if (idx as f64) != value {
                    return raise_exception::<_>(
                        _py,
                        "TypeError",
                        &format!("take_rows indices must be integers, got {:?}", value),
                    );
                }
                idx
            } else {
                return raise_exception::<_>(
                    _py,
                    "TypeError",
                    "take_rows indices must be integers",
                );
            };
            let mut resolved = idx;
            if resolved < 0 && allow_negative {
                resolved += x_shape[0] as i64;
            }
            if resolved < 0 || resolved >= x_shape[0] as i64 {
                return raise_exception::<_>(
                    _py,
                    "IndexError",
                    &format!(
                        "Index {} out of range for axis 0 with size {}",
                        idx, x_shape[0]
                    ),
                );
            }
            let src_start = resolved as usize * width;
            let dst_start = out_row * width;
            out[dst_start..dst_start + width].copy_from_slice(&src[src_start..src_start + width]);
        }
        let out_data_ptr = alloc_bytearray(_py, out.as_slice());
        if out_data_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let mut out_shape = indices_shape.clone();
        out_shape.extend_from_slice(row_shape);
        let out_shape_bits = match alloc_tuple_bits_from_usize(_py, out_shape.as_slice()) {
            Ok(bits) => bits,
            Err(bits) => {
                crate::dec_ref_bits(_py, MoltObject::from_ptr(out_data_ptr).bits());
                return bits;
            }
        };
        let out_data_bits = MoltObject::from_ptr(out_data_ptr).bits();
        let tensor_bits = match unsafe {
            build_tensor_from_data_bits(
                _py,
                x.class_bits,
                x.buffer.class_bits,
                out_data_bits,
                x.buffer.element_type_bits,
                rows.len() * row_size,
                x.buffer.format_bits,
                x.buffer.format.itemsize(),
                out_shape_bits,
                x.dtype_bits,
            )
        } {
            Ok(bits) => bits,
            Err(bits) => bits,
        };
        crate::dec_ref_bits(_py, out_data_bits);
        crate::dec_ref_bits(_py, out_shape_bits);
        tensor_bits
    })
}

#[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
#[allow(non_snake_case)]
pub extern "C" fn molt_gpu_tensor__zeros(shape_bits: u64, dtype_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let tensor_class_bits =
            match unsafe { module_global_bits(_py, b"molt.gpu.tensor", b"Tensor", "Tensor") } {
                Ok(bits) => bits,
                Err(bits) => return bits,
            };
        let buffer_class_bits =
            match unsafe { module_global_bits(_py, b"molt.gpu.tensor", b"Buffer", "Buffer") } {
                Ok(bits) => bits,
                Err(bits) => {
                    crate::dec_ref_bits(_py, tensor_class_bits);
                    return bits;
                }
            };
        let dims_i64 = match parse_i64_sequence_arg(_py, shape_bits, "shape", true) {
            Ok(value) => value,
            Err(bits) => {
                crate::dec_ref_bits(_py, tensor_class_bits);
                crate::dec_ref_bits(_py, buffer_class_bits);
                return bits;
            }
        };
        let mut dims = Vec::with_capacity(dims_i64.len());
        for dim in dims_i64 {
            let value = usize::try_from(dim).map_err(|_| {
                raise_exception::<u64>(_py, "ValueError", "shape dimensions must be non-negative")
            });
            match value {
                Ok(value) => dims.push(value),
                Err(bits) => {
                    crate::dec_ref_bits(_py, tensor_class_bits);
                    crate::dec_ref_bits(_py, buffer_class_bits);
                    return bits;
                }
            }
        }
        let size = product(dims.as_slice());
        let out = vec![0u8; size * 8];
        let data_ptr = alloc_bytearray(_py, out.as_slice());
        if data_ptr.is_null() {
            crate::dec_ref_bits(_py, tensor_class_bits);
            crate::dec_ref_bits(_py, buffer_class_bits);
            return MoltObject::none().bits();
        }
        let data_bits = MoltObject::from_ptr(data_ptr).bits();
        let format_bits = match alloc_string_bits(_py, b"d") {
            Ok(bits) => bits,
            Err(bits) => {
                crate::dec_ref_bits(_py, tensor_class_bits);
                crate::dec_ref_bits(_py, buffer_class_bits);
                crate::dec_ref_bits(_py, data_bits);
                return bits;
            }
        };
        let shape_tuple_bits = match alloc_tuple_bits_from_usize(_py, dims.as_slice()) {
            Ok(bits) => bits,
            Err(bits) => {
                crate::dec_ref_bits(_py, tensor_class_bits);
                crate::dec_ref_bits(_py, buffer_class_bits);
                crate::dec_ref_bits(_py, data_bits);
                crate::dec_ref_bits(_py, format_bits);
                return bits;
            }
        };
        let tensor_bits = match unsafe {
            build_tensor_from_data_bits(
                _py,
                tensor_class_bits,
                buffer_class_bits,
                data_bits,
                crate::builtins::classes::builtin_classes(_py).float,
                size,
                format_bits,
                ScalarFormat::F64.itemsize(),
                shape_tuple_bits,
                dtype_bits,
            )
        } {
            Ok(bits) => bits,
            Err(bits) => bits,
        };
        crate::dec_ref_bits(_py, tensor_class_bits);
        crate::dec_ref_bits(_py, buffer_class_bits);
        crate::dec_ref_bits(_py, data_bits);
        crate::dec_ref_bits(_py, format_bits);
        crate::dec_ref_bits(_py, shape_tuple_bits);
        tensor_bits
    })
}

#[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
#[allow(non_snake_case)]
pub extern "C" fn molt_gpu_tensor__tensor_attention_hybrid_mask(
    token_ids_bits: u64,
    image_cls_id_bits: u64,
    img_end_id_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let tensor_class_bits =
            match unsafe { module_global_bits(_py, b"molt.gpu.tensor", b"Tensor", "Tensor") } {
                Ok(bits) => bits,
                Err(bits) => return bits,
            };
        let buffer_class_bits =
            match unsafe { module_global_bits(_py, b"molt.gpu.tensor", b"Buffer", "Buffer") } {
                Ok(bits) => bits,
                Err(bits) => {
                    crate::dec_ref_bits(_py, tensor_class_bits);
                    return bits;
                }
            };
        let token_ids = match parse_i64_sequence_arg(_py, token_ids_bits, "token_ids", false) {
            Ok(value) => value,
            Err(bits) => {
                crate::dec_ref_bits(_py, tensor_class_bits);
                crate::dec_ref_bits(_py, buffer_class_bits);
                return bits;
            }
        };
        let Some(image_cls_id) = to_i64(obj_from_bits(image_cls_id_bits)) else {
            crate::dec_ref_bits(_py, tensor_class_bits);
            crate::dec_ref_bits(_py, buffer_class_bits);
            return raise_exception::<_>(_py, "TypeError", "image_cls_id must be an int");
        };
        let Some(img_end_id) = to_i64(obj_from_bits(img_end_id_bits)) else {
            crate::dec_ref_bits(_py, tensor_class_bits);
            crate::dec_ref_bits(_py, buffer_class_bits);
            return raise_exception::<_>(_py, "TypeError", "img_end_id must be an int");
        };

        let len = token_ids.len();
        let mut in_block = vec![false; len];
        let mut block_idx = vec![0usize; len];
        let mut block_starts: Vec<usize> = Vec::new();
        let mut block_ends: Vec<usize> = Vec::new();
        let mut depth = 0i64;
        let mut current_block = 0usize;
        for (i, tid) in token_ids.iter().copied().enumerate() {
            let is_soi = tid == image_cls_id;
            let is_eoi = tid == img_end_id;
            if is_soi {
                depth += 1;
                current_block += 1;
                block_starts.push(i);
                block_ends.push(i + 1);
            }
            if depth > 0 {
                in_block[i] = true;
                let block = current_block - 1;
                block_idx[i] = block;
                block_ends[block] = i + 1;
            }
            if is_eoi && depth > 0 {
                depth -= 1;
            }
        }

        let mut values = vec![-1.0e9f32; len * len];
        for q in 0..len {
            let row = &mut values[q * len..(q + 1) * len];
            row[..=q].fill(0.0);
            if in_block[q] {
                let block = block_idx[q];
                row[block_starts[block]..block_ends[block]].fill(0.0);
            }
        }

        let raw = unsafe {
            std::slice::from_raw_parts(
                values.as_ptr() as *const u8,
                values.len() * std::mem::size_of::<f32>(),
            )
        };
        let data_ptr = alloc_bytearray(_py, raw);
        if data_ptr.is_null() {
            crate::dec_ref_bits(_py, tensor_class_bits);
            crate::dec_ref_bits(_py, buffer_class_bits);
            return MoltObject::none().bits();
        }
        let data_bits = MoltObject::from_ptr(data_ptr).bits();
        let format_bits = match alloc_string_bits(_py, b"f") {
            Ok(bits) => bits,
            Err(bits) => {
                crate::dec_ref_bits(_py, tensor_class_bits);
                crate::dec_ref_bits(_py, buffer_class_bits);
                crate::dec_ref_bits(_py, data_bits);
                return bits;
            }
        };
        let shape_bits = match alloc_tuple_bits_from_usize(_py, &[1, 1, len, len]) {
            Ok(bits) => bits,
            Err(bits) => {
                crate::dec_ref_bits(_py, tensor_class_bits);
                crate::dec_ref_bits(_py, buffer_class_bits);
                crate::dec_ref_bits(_py, data_bits);
                crate::dec_ref_bits(_py, format_bits);
                return bits;
            }
        };
        let float_bits = crate::builtins::classes::builtin_classes(_py).float;
        let tensor_bits = match unsafe {
            build_tensor_from_data_bits(
                _py,
                tensor_class_bits,
                buffer_class_bits,
                data_bits,
                float_bits,
                values.len(),
                format_bits,
                ScalarFormat::F32.itemsize(),
                shape_bits,
                float_bits,
            )
        } {
            Ok(bits) => bits,
            Err(bits) => bits,
        };
        crate::dec_ref_bits(_py, tensor_class_bits);
        crate::dec_ref_bits(_py, buffer_class_bits);
        crate::dec_ref_bits(_py, data_bits);
        crate::dec_ref_bits(_py, format_bits);
        crate::dec_ref_bits(_py, shape_bits);
        tensor_bits
    })
}

#[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
#[allow(non_snake_case)]
pub extern "C" fn molt_gpu_tensor__tensor_scaled_dot_product_attention(
    q_bits: u64,
    k_bits: u64,
    v_bits: u64,
    mask_bits: u64,
    scale_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let (q, q_shape) = match unsafe { tensor_runtime_view(_py, q_bits, "q") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let (k, k_shape) = match unsafe { tensor_runtime_view(_py, k_bits, "k") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let (v, v_shape) = match unsafe { tensor_runtime_view(_py, v_bits, "v") } {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if q_shape.len() != 4 || k_shape.len() != 4 || v_shape.len() != 4 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "scaled dot product attention requires rank-4 tensors",
            );
        }

        let batch = q_shape[0];
        let heads = q_shape[1];
        let seq_q = q_shape[2];
        let dim = q_shape[3];
        let seq_k = k_shape[2];
        let value_dim = v_shape[3];
        if k_shape[0] != batch
            || k_shape[1] != heads
            || k_shape[3] != dim
            || v_shape[0] != batch
            || v_shape[1] != heads
            || v_shape[2] != seq_k
        {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "scaled dot product attention shape mismatch",
            );
        }
        if q.buffer.format != ScalarFormat::F32
            || k.buffer.format != ScalarFormat::F32
            || v.buffer.format != ScalarFormat::F32
        {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "scaled dot product attention currently requires f32 tensors",
            );
        }

        let scale = if let Some(value) = to_f64(obj_from_bits(scale_bits)) {
            value as f32
        } else if let Some(value) = to_i64(obj_from_bits(scale_bits)) {
            value as f32
        } else {
            return raise_exception::<_>(_py, "TypeError", "scale must be a float");
        };

        let q_total = product(&q_shape);
        let k_total = product(&k_shape);
        let v_total = product(&v_shape);
        let Some(q_required) = q_total.checked_mul(ScalarFormat::F32.itemsize()) else {
            return raise_exception::<_>(_py, "OverflowError", "q shape overflow");
        };
        let Some(k_required) = k_total.checked_mul(ScalarFormat::F32.itemsize()) else {
            return raise_exception::<_>(_py, "OverflowError", "k shape overflow");
        };
        let Some(v_required) = v_total.checked_mul(ScalarFormat::F32.itemsize()) else {
            return raise_exception::<_>(_py, "OverflowError", "v shape overflow");
        };
        if q.buffer.data_view.len < q_required
            || k.buffer.data_view.len < k_required
            || v.buffer.data_view.len < v_required
        {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "scaled dot product attention input buffer is too small",
            );
        }

        let mask_info = if obj_from_bits(mask_bits).is_none() {
            None
        } else {
            let (mask, mask_shape) = match unsafe { tensor_runtime_view(_py, mask_bits, "mask") } {
                Ok(value) => value,
                Err(bits) => return bits,
            };
            if mask_shape.len() != 4 || mask.buffer.format != ScalarFormat::F32 {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "scaled dot product attention mask must be a rank-4 f32 tensor",
                );
            }
            let expected = [batch, heads, seq_q, seq_k];
            for (dim_value, expected_value) in mask_shape.iter().zip(expected.iter()) {
                if *dim_value != 1 && *dim_value != *expected_value {
                    return raise_exception::<_>(
                        _py,
                        "ValueError",
                        "scaled dot product attention mask shape mismatch",
                    );
                }
            }
            let mask_total = product(&mask_shape);
            let Some(mask_required) = mask_total.checked_mul(ScalarFormat::F32.itemsize()) else {
                return raise_exception::<_>(_py, "OverflowError", "mask shape overflow");
            };
            if mask.buffer.data_view.len < mask_required {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "scaled dot product attention mask buffer is too small",
                );
            }
            let mask_strides = strides(&mask_shape);
            Some((mask, mask_shape, mask_strides))
        };

        let Some(out_elems) = batch
            .checked_mul(heads)
            .and_then(|n| n.checked_mul(seq_q))
            .and_then(|n| n.checked_mul(value_dim))
        else {
            return raise_exception::<_>(_py, "OverflowError", "attention output shape overflow");
        };
        let mut out = vec![0u8; out_elems * ScalarFormat::F32.itemsize()];
        let q_stride = seq_q * dim;
        let k_stride = seq_k * dim;
        let v_stride = seq_k * value_dim;
        let out_stride = seq_q * value_dim;
        for b in 0..batch {
            for h in 0..heads {
                let q_batch_off = (b * heads + h) * q_stride;
                let k_batch_off = (b * heads + h) * k_stride;
                let v_batch_off = (b * heads + h) * v_stride;
                let out_batch_off = (b * heads + h) * out_stride;
                for q_idx in 0..seq_q {
                    let q_base = q_batch_off + q_idx * dim;
                    let mut max_score = f32::NEG_INFINITY;
                    for k_idx in 0..seq_k {
                        let k_base = k_batch_off + k_idx * dim;
                        let mut score = 0.0f32;
                        for d in 0..dim {
                            let qv = unsafe {
                                (q.buffer.data_view.ptr.add((q_base + d) * 4) as *const f32)
                                    .read_unaligned()
                            };
                            let kv = unsafe {
                                (k.buffer.data_view.ptr.add((k_base + d) * 4) as *const f32)
                                    .read_unaligned()
                            };
                            score += qv * kv;
                        }
                        score *= scale;
                        if let Some((mask, mask_shape, mask_strides)) = &mask_info {
                            let mask_index =
                                (if mask_shape[0] == 1 { 0 } else { b * mask_strides[0] })
                                + (if mask_shape[1] == 1 {
                                    0
                                } else {
                                    h * mask_strides[1]
                                })
                                + (if mask_shape[2] == 1 {
                                    0
                                } else {
                                    q_idx * mask_strides[2]
                                })
                                + (if mask_shape[3] == 1 {
                                    0
                                } else {
                                    k_idx * mask_strides[3]
                                });
                            score += unsafe {
                                (mask.buffer.data_view.ptr.add(mask_index * 4) as *const f32)
                                    .read_unaligned()
                            };
                        }
                        if score > max_score {
                            max_score = score;
                        }
                    }

                    let mut sum = 0.0f32;
                    let mut acc = vec![0.0f32; value_dim];
                    for k_idx in 0..seq_k {
                        let k_base = k_batch_off + k_idx * dim;
                        let mut score = 0.0f32;
                        for d in 0..dim {
                            let qv = unsafe {
                                (q.buffer.data_view.ptr.add((q_base + d) * 4) as *const f32)
                                    .read_unaligned()
                            };
                            let kv = unsafe {
                                (k.buffer.data_view.ptr.add((k_base + d) * 4) as *const f32)
                                    .read_unaligned()
                            };
                            score += qv * kv;
                        }
                        score *= scale;
                        if let Some((mask, mask_shape, mask_strides)) = &mask_info {
                            let mask_index =
                                (if mask_shape[0] == 1 { 0 } else { b * mask_strides[0] })
                                + (if mask_shape[1] == 1 {
                                    0
                                } else {
                                    h * mask_strides[1]
                                })
                                + (if mask_shape[2] == 1 {
                                    0
                                } else {
                                    q_idx * mask_strides[2]
                                })
                                + (if mask_shape[3] == 1 {
                                    0
                                } else {
                                    k_idx * mask_strides[3]
                                });
                            score += unsafe {
                                (mask.buffer.data_view.ptr.add(mask_index * 4) as *const f32)
                                    .read_unaligned()
                            };
                        }
                        let weight = (score - max_score).exp();
                        sum += weight;
                        let v_base = v_batch_off + k_idx * value_dim;
                        for d in 0..value_dim {
                            let vv = unsafe {
                                (v.buffer.data_view.ptr.add((v_base + d) * 4) as *const f32)
                                    .read_unaligned()
                            };
                            acc[d] += weight * vv;
                        }
                    }

                    let inv_sum = if sum != 0.0 { 1.0 / sum } else { 0.0 };
                    let out_base = out_batch_off + q_idx * value_dim;
                    for d in 0..value_dim {
                        unsafe {
                            (out.as_mut_ptr().add((out_base + d) * 4) as *mut f32)
                                .write_unaligned(acc[d] * inv_sum);
                        }
                    }
                }
            }
        }

        let data_ptr = alloc_bytearray(_py, out.as_slice());
        if data_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let data_bits = MoltObject::from_ptr(data_ptr).bits();
        let format_bits = match alloc_string_bits(_py, b"f") {
            Ok(bits) => bits,
            Err(bits) => {
                crate::dec_ref_bits(_py, data_bits);
                return bits;
            }
        };
        let shape_bits = match alloc_tuple_bits_from_usize(_py, &[batch, heads, seq_q, value_dim]) {
            Ok(bits) => bits,
            Err(bits) => {
                crate::dec_ref_bits(_py, data_bits);
                crate::dec_ref_bits(_py, format_bits);
                return bits;
            }
        };
        let tensor_bits = match unsafe {
            build_tensor_from_data_bits(
                _py,
                q.class_bits,
                q.buffer.class_bits,
                data_bits,
                crate::builtins::classes::builtin_classes(_py).float,
                out_elems,
                format_bits,
                ScalarFormat::F32.itemsize(),
                shape_bits,
                q.dtype_bits,
            )
        } {
            Ok(bits) => bits,
            Err(bits) => bits,
        };
        crate::dec_ref_bits(_py, data_bits);
        crate::dec_ref_bits(_py, format_bits);
        crate::dec_ref_bits(_py, shape_bits);
        tensor_bits
    })
}

#[cfg_attr(target_arch = "wasm32", unsafe(no_mangle))]
#[allow(non_snake_case)]
pub extern "C" fn molt_gpu_interop__load_safetensors(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let load_bits = match unsafe {
            module_global_bits(
                _py,
                b"molt.gpu.interop",
                b"load_safetensors",
                "load_safetensors",
            )
        } {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let out_bits = unsafe { crate::call::dispatch::call_callable1(_py, load_bits, path_bits) };
        crate::dec_ref_bits(_py, load_bits);
        out_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_tensor_from_parts(
    tensor_class_bits: u64,
    buffer_class_bits: u64,
    data_bits: u64,
    element_type_bits: u64,
    size_bits: u64,
    format_bits: u64,
    shape_bits: u64,
    dtype_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let size = match parse_usize_arg(_py, size_bits, "size") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let format = match parse_format(_py, format_bits, "format_char") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let data_view = match bytes_like_view(_py, data_bits, "data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };
        let Some(required_len) = size.checked_mul(format.itemsize()) else {
            return raise_exception::<_>(_py, "OverflowError", "buffer size overflow");
        };
        if data_view.len < required_len {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "Buffer payload too small for requested size and format",
            );
        }
        let (shape_bits, owns_shape_bits) = match normalize_shape_bits(_py, shape_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let buffer_bits = match unsafe {
            build_buffer_instance(
                _py,
                buffer_class_bits,
                data_bits,
                element_type_bits,
                size,
                format_bits,
                format.itemsize(),
            )
        } {
            Ok(bits) => bits,
            Err(bits) => {
                if owns_shape_bits {
                    crate::dec_ref_bits(_py, shape_bits);
                }
                return bits;
            }
        };
        let tensor_bits = match unsafe {
            build_tensor_instance(_py, tensor_class_bits, buffer_bits, shape_bits, dtype_bits)
        } {
            Ok(bits) => bits,
            Err(bits) => {
                crate::dec_ref_bits(_py, buffer_bits);
                if owns_shape_bits {
                    crate::dec_ref_bits(_py, shape_bits);
                }
                return bits;
            }
        };
        crate::dec_ref_bits(_py, buffer_bits);
        if owns_shape_bits {
            crate::dec_ref_bits(_py, shape_bits);
        }
        tensor_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_repeat_axis_contiguous(
    x_data_bits: u64,
    x_format_bits: u64,
    shape_bits: u64,
    axis_bits: u64,
    repeats_bits: u64,
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
                "repeat_axis requires matching input/output formats",
            );
        }
        let shape = match parse_shape(_py, shape_bits, "shape") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if shape.is_empty() {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "repeat_axis requires a tensor with at least 1 dimension",
            );
        }
        let axis = match parse_usize_arg(_py, axis_bits, "axis") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if axis >= shape.len() {
            return raise_exception::<_>(
                _py,
                "ValueError",
                &format!("Invalid axis {} for tensor with {} dims", axis, shape.len()),
            );
        }
        let repeats = match parse_usize_arg(_py, repeats_bits, "repeats") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let x_view = match bytes_like_view(_py, x_data_bits, "x_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };
        let total_elems = product(&shape);
        let itemsize = x_format.itemsize();
        let Some(required) = total_elems.checked_mul(itemsize) else {
            return raise_exception::<_>(_py, "OverflowError", "repeat_axis shape overflow");
        };
        if x_view.len < required {
            return raise_exception::<_>(_py, "ValueError", "x_data buffer is too small");
        }

        let outer = if axis > 0 {
            product(&shape[..axis])
        } else {
            1
        };
        let axis_len = shape[axis];
        let inner = if axis + 1 < shape.len() {
            product(&shape[axis + 1..])
        } else {
            1
        };
        let Some(chunk_bytes) = inner.checked_mul(itemsize) else {
            return raise_exception::<_>(_py, "OverflowError", "repeat_axis byte size overflow");
        };
        let Some(src_axis_bytes) = axis_len.checked_mul(chunk_bytes) else {
            return raise_exception::<_>(_py, "OverflowError", "repeat_axis byte size overflow");
        };
        let Some(out_axis_len) = axis_len.checked_mul(repeats) else {
            return raise_exception::<_>(_py, "OverflowError", "repeat_axis output shape overflow");
        };
        let Some(out_axis_bytes) = out_axis_len.checked_mul(chunk_bytes) else {
            return raise_exception::<_>(_py, "OverflowError", "repeat_axis byte size overflow");
        };
        let Some(out_len) = outer.checked_mul(out_axis_bytes) else {
            return raise_exception::<_>(
                _py,
                "OverflowError",
                "repeat_axis output byte size overflow",
            );
        };
        let mut out = vec![0u8; out_len];
        let src = unsafe { std::slice::from_raw_parts(x_view.ptr, required) };
        for outer_idx in 0..outer {
            let src_outer = outer_idx * src_axis_bytes;
            let dst_outer = outer_idx * out_axis_bytes;
            for axis_idx in 0..axis_len {
                let src_base = src_outer + axis_idx * chunk_bytes;
                let chunk = &src[src_base..src_base + chunk_bytes];
                let dst_base = dst_outer + axis_idx * repeats * chunk_bytes;
                for repeat_idx in 0..repeats {
                    let dst = dst_base + repeat_idx * chunk_bytes;
                    out[dst..dst + chunk_bytes].copy_from_slice(chunk);
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
pub extern "C" fn molt_gpu_tensor_from_buffer(
    tensor_class_bits: u64,
    buffer_bits: u64,
    shape_bits: u64,
    dtype_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let (shape_bits, owns_shape_bits) = match normalize_shape_bits(_py, shape_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let tensor_bits = match unsafe {
            build_tensor_instance(_py, tensor_class_bits, buffer_bits, shape_bits, dtype_bits)
        } {
            Ok(bits) => bits,
            Err(bits) => {
                if owns_shape_bits {
                    crate::dec_ref_bits(_py, shape_bits);
                }
                return bits;
            }
        };
        if owns_shape_bits {
            crate::dec_ref_bits(_py, shape_bits);
        }
        tensor_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gpu_buffer_to_list(buffer_bits: u64, count_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let count = match parse_usize_arg(_py, count_bits, "count") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let data_bits = match unsafe { object_attr_bits(_py, buffer_bits, b"_data", "_data") } {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let format_bits =
            match unsafe { object_attr_bits(_py, buffer_bits, b"_format_char", "_format_char") } {
                Ok(bits) => bits,
                Err(bits) => return bits,
            };
        let format = match parse_format(_py, format_bits, "format_char") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let data_view = match bytes_like_view(_py, data_bits, "_data") {
            Ok(view) => view,
            Err(bits) => return bits,
        };
        let Some(required_len) = count.checked_mul(format.itemsize()) else {
            return raise_exception::<_>(_py, "OverflowError", "buffer list size overflow");
        };
        if data_view.len < required_len {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "Buffer payload too small for requested count and format",
            );
        }
        let mut values = Vec::with_capacity(count);
        for index in 0..count {
            let bits = match format {
                ScalarFormat::F32 | ScalarFormat::F64 => {
                    MoltObject::from_float(unsafe { read_scalar(data_view.ptr, index, format) })
                        .bits()
                }
                ScalarFormat::I64 => MoltObject::from_int(unsafe {
                    read_scalar(data_view.ptr, index, format) as i64
                })
                .bits(),
            };
            values.push(bits);
        }
        let list_ptr =
            crate::object::builders::alloc_list_with_capacity_owned(_py, &values, values.len());
        if list_ptr.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(list_ptr).bits()
    })
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
                linear_rows_f32(
                    x_view.ptr,
                    weight_view.ptr,
                    out.as_mut_ptr(),
                    outer,
                    in_features,
                    0,
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
        let Some(out_features) = split_sizes
            .iter()
            .try_fold(0usize, |acc, size| acc.checked_add(*size))
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
pub extern "C" fn molt_gpu_linear_squared_relu_gate_interleaved_contiguous(
    x_data_bits: u64,
    x_format_bits: u64,
    weight_data_bits: u64,
    weight_format_bits: u64,
    outer_bits: u64,
    in_features_bits: u64,
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
        if x_view.len < x_required {
            return raise_exception::<_>(_py, "ValueError", "x_data buffer is too small");
        }
        if in_features == 0 {
            return raise_exception::<_>(_py, "ValueError", "in_features must be positive");
        }
        let row_bytes = in_features * weight_format.itemsize();
        if row_bytes == 0 || weight_view.len % row_bytes != 0 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "weight_data byte length must be an even multiple of row width",
            );
        }
        let out_features = weight_view.len / row_bytes;
        if out_features % 2 != 0 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "interleaved gate weight output dimension must be even",
            );
        }
        let hidden = out_features / 2;
        let Some(out_len) = outer
            .checked_mul(hidden)
            .and_then(|n| n.checked_mul(out_format.itemsize()))
        else {
            return raise_exception::<_>(_py, "OverflowError", "output shape overflow");
        };
        let mut out = vec![0u8; out_len];

        if x_format == ScalarFormat::F32
            && weight_format == ScalarFormat::F32
            && out_format == ScalarFormat::F32
        {
            unsafe {
                linear_squared_relu_gate_interleaved_f32(
                    x_view.ptr,
                    weight_view.ptr,
                    out.as_mut_ptr(),
                    outer,
                    in_features,
                    hidden,
                );
            }
        } else {
            for batch in 0..outer {
                let x_off = batch * in_features;
                let out_off = batch * hidden;
                for hidden_idx in 0..hidden {
                    let gate_off = (2 * hidden_idx) * in_features;
                    let up_off = (2 * hidden_idx + 1) * in_features;
                    let mut gate = 0.0f64;
                    let mut up = 0.0f64;
                    for k in 0..in_features {
                        let x = unsafe { read_scalar(x_view.ptr, x_off + k, x_format) };
                        let gate_w =
                            unsafe { read_scalar(weight_view.ptr, gate_off + k, weight_format) };
                        let up_w =
                            unsafe { read_scalar(weight_view.ptr, up_off + k, weight_format) };
                        gate += x * gate_w;
                        up += x * up_w;
                    }
                    let relu = gate.max(0.0);
                    unsafe {
                        write_scalar(
                            out.as_mut_ptr(),
                            out_off + hidden_idx,
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
                return raise_exception::<_>(_py, "ValueError", "Cannot broadcast input shapes");
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
                return raise_exception::<_>(_py, "ValueError", "matmul batch shape mismatch");
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
                            write_scalar(
                                out.as_mut_ptr(),
                                out_off + i * b_cols + j,
                                out_format,
                                acc,
                            )
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
        let Some(freq_required) = seq_len.checked_mul(freq_dim).and_then(|n| n.checked_mul(4))
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
                                    unsafe {
                                        read_scalar(cos_view.ptr, freq_base + i, ScalarFormat::F32)
                                    },
                                    unsafe {
                                        read_scalar(sin_view.ptr, freq_base + i, ScalarFormat::F32)
                                    },
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
            out[dst_base..dst_base + itemsize].copy_from_slice(&src[src_base..src_base + itemsize]);
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
        let outer = if axis_len == 0 {
            0
        } else {
            total_elems / axis_len
        };
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
            return raise_exception::<_>(_py, "ValueError", "rms_norm last axis must be non-empty");
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
                rms_norm_last_axis_f32(x_view.ptr, out.as_mut_ptr(), outer, axis_len, eps as f32)
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
        let outer = if axis_len == 0 {
            0
        } else {
            total_elems / axis_len
        };
        let out_elems = outer * (axis_len / 2);
        let mut out = vec![0u8; out_elems * out_format.itemsize()];
        if x_format == ScalarFormat::F32 && out_format == ScalarFormat::F32 {
            unsafe {
                squared_relu_gate_interleaved_f32(x_view.ptr, out.as_mut_ptr(), outer, axis_len)
            };
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
                        write_scalar(out.as_mut_ptr(), out_base + i, out_format, relu * relu * up)
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
    #[cfg(target_arch = "aarch64")]
    use super::{
        linear_dot4_gate_up_interleaved_unaligned, linear_dot4_rows_unaligned,
        linear_dot8_gate_up_interleaved_unaligned, linear_gate_up8_store_unaligned,
    };
    #[cfg(any(
        target_arch = "aarch64",
        target_arch = "x86_64",
        all(target_arch = "wasm32", target_feature = "simd128")
    ))]
    use super::{linear_gate_up4_store_unaligned, linear_rows4_store_ptrs_unaligned};
    use super::{
        molt_gpu_broadcast_binary_contiguous, molt_gpu_buffer_to_list, molt_gpu_linear_contiguous,
        molt_gpu_linear_split_last_dim_contiguous,
        molt_gpu_linear_squared_relu_gate_interleaved_contiguous, molt_gpu_matmul_contiguous,
        molt_gpu_repeat_axis_contiguous, molt_gpu_rms_norm_last_axis_contiguous,
        molt_gpu_rope_apply_contiguous,
        molt_gpu_softmax_last_axis_contiguous, molt_gpu_squared_relu_gate_interleaved_contiguous,
        molt_gpu_tensor__tensor_attention_hybrid_mask, molt_gpu_tensor__tensor_data_list, molt_gpu_tensor__tensor_linear,
        molt_gpu_tensor__tensor_reshape_view, molt_gpu_tensor__zeros, molt_gpu_tensor_from_parts,
    };
    use crate::{
        MoltObject, alloc_bytes, alloc_class_obj, alloc_string, alloc_tuple,
        attr_name_bits_from_bytes, builtin_classes, bytes_data, bytes_len, dec_ref_bits,
        obj_from_bits, seq_vec_ref, to_f64,
    };

    fn f32_bytes(values: &[f32]) -> Vec<u8> {
        let mut out = Vec::with_capacity(values.len() * 4);
        for value in values {
            out.extend_from_slice(&value.to_ne_bytes());
        }
        out
    }

    fn make_tensor_from_f32(
        _py: &crate::PyToken<'_>,
        tensor_cls_bits: u64,
        buffer_cls_bits: u64,
        values: &[f32],
        shape: &[i64],
    ) -> u64 {
        let data_ptr = alloc_bytes(_py, &f32_bytes(values));
        let fmt_ptr = alloc_string(_py, b"f");
        let shape_bits: Vec<u64> = shape
            .iter()
            .copied()
            .map(|dim| MoltObject::from_int(dim).bits())
            .collect();
        let shape_ptr = alloc_tuple(_py, shape_bits.as_slice());
        molt_gpu_tensor_from_parts(
            tensor_cls_bits,
            buffer_cls_bits,
            MoltObject::from_ptr(data_ptr).bits(),
            builtin_classes(_py).float,
            MoltObject::from_int(values.len() as i64).bits(),
            MoltObject::from_ptr(fmt_ptr).bits(),
            MoltObject::from_ptr(shape_ptr).bits(),
            builtin_classes(_py).float,
        )
    }

    fn attr_bits(_py: &crate::PyToken<'_>, obj_bits: u64, name: &[u8]) -> u64 {
        let name_bits = attr_name_bits_from_bytes(_py, name).expect("attr name");
        let value_bits = crate::molt_get_attr_name(obj_bits, name_bits);
        dec_ref_bits(_py, name_bits);
        value_bits
    }

    fn install_gpu_tensor_module(
        _py: &crate::PyToken<'_>,
        tensor_cls_bits: u64,
        buffer_cls_bits: u64,
    ) {
        let root_name_ptr = alloc_string(_py, b"molt");
        let gpu_name_ptr = alloc_string(_py, b"molt.gpu");
        let tensor_name_ptr = alloc_string(_py, b"molt.gpu.tensor");
        let root_name_bits = MoltObject::from_ptr(root_name_ptr).bits();
        let gpu_name_bits = MoltObject::from_ptr(gpu_name_ptr).bits();
        let tensor_name_bits_full = MoltObject::from_ptr(tensor_name_ptr).bits();
        let root_module_bits = crate::builtins::modules::molt_module_new(root_name_bits);
        let gpu_module_bits = crate::builtins::modules::molt_module_new(gpu_name_bits);
        let tensor_module_bits = crate::builtins::modules::molt_module_new(tensor_name_bits_full);
        assert!(!crate::exception_pending(_py));

        let gpu_attr_bits = attr_name_bits_from_bytes(_py, b"gpu").expect("gpu attr");
        let tensor_attr_bits =
            attr_name_bits_from_bytes(_py, b"tensor").expect("tensor attr");
        let tensor_name_bits = attr_name_bits_from_bytes(_py, b"Tensor").expect("Tensor attr");
        let buffer_name_bits = attr_name_bits_from_bytes(_py, b"Buffer").expect("Buffer attr");
        crate::builtins::modules::molt_module_set_attr(
            root_module_bits,
            gpu_attr_bits,
            gpu_module_bits,
        );
        crate::builtins::modules::molt_module_set_attr(
            gpu_module_bits,
            tensor_attr_bits,
            tensor_module_bits,
        );
        crate::builtins::modules::molt_module_set_attr(
            tensor_module_bits,
            tensor_name_bits,
            tensor_cls_bits,
        );
        crate::builtins::modules::molt_module_set_attr(
            tensor_module_bits,
            buffer_name_bits,
            buffer_cls_bits,
        );
        crate::builtins::modules::molt_module_cache_set(root_name_bits, root_module_bits);
        crate::builtins::modules::molt_module_cache_set(gpu_name_bits, gpu_module_bits);
        crate::builtins::modules::molt_module_cache_set(tensor_name_bits_full, tensor_module_bits);
        dec_ref_bits(_py, gpu_attr_bits);
        dec_ref_bits(_py, tensor_attr_bits);
        dec_ref_bits(_py, tensor_name_bits);
        dec_ref_bits(_py, buffer_name_bits);
        dec_ref_bits(_py, root_name_bits);
        dec_ref_bits(_py, gpu_name_bits);
        dec_ref_bits(_py, tensor_name_bits_full);
        dec_ref_bits(_py, root_module_bits);
        dec_ref_bits(_py, gpu_module_bits);
        dec_ref_bits(_py, tensor_module_bits);
        assert!(!crate::exception_pending(_py));
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn linear_dot4_rows_unaligned_matches_scalar_rows() {
        let x = [1.5f32, -2.0, 0.5, 3.0, -1.0, 4.0];
        let weights = [
            0.25f32, 1.0, -0.5, 2.0, 0.0, 1.5, -1.0, 0.5, 0.75, -0.25, 1.25, 0.0, 2.0, -0.5, 1.0,
            0.0, -1.5, 0.5, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0,
        ];
        let mut x_bytes = vec![0u8];
        x_bytes.extend_from_slice(&f32_bytes(&x));
        let mut weight_bytes = vec![0u8, 0u8, 0u8];
        weight_bytes.extend_from_slice(&f32_bytes(&weights));
        let row_offsets = [0usize, 6, 12, 18];

        let got = unsafe {
            linear_dot4_rows_unaligned(
                x_bytes[1..].as_ptr(),
                0,
                weight_bytes[3..].as_ptr(),
                row_offsets,
                x.len(),
            )
        };

        for (row_idx, row_off) in row_offsets.into_iter().enumerate() {
            let expected = x
                .iter()
                .zip(weights[row_off..row_off + x.len()].iter())
                .map(|(lhs, rhs)| lhs * rhs)
                .sum::<f32>();
            assert!(
                (got[row_idx] - expected).abs() < 1e-5,
                "row {row_idx} mismatch: got {}, expected {expected}",
                got[row_idx]
            );
        }
    }

    #[cfg(any(
        target_arch = "aarch64",
        target_arch = "x86_64",
        all(target_arch = "wasm32", target_feature = "simd128")
    ))]
    #[test]
    fn linear_rows4_store_unaligned_matches_scalar_rows() {
        let x = [1.5f32, -2.0, 0.5, 3.0, -1.0, 4.0];
        let weights = [
            0.25f32, 1.0, -0.5, 2.0, 0.0, 1.5, -1.0, 0.5, 0.75, -0.25, 1.25, 0.0, 2.0, -0.5, 1.0,
            0.0, -1.5, 0.5, 0.0, 1.0, 0.0, 1.0, 0.0, 1.0,
        ];
        let row_offsets = [0usize, 6, 12, 18];
        let mut x_bytes = vec![0u8];
        x_bytes.extend_from_slice(&f32_bytes(&x));
        let mut weight_bytes = vec![0u8, 0u8, 0u8];
        weight_bytes.extend_from_slice(&f32_bytes(&weights));
        let mut out_bytes = vec![0u8; 4 * 4 + 1];

        unsafe {
            linear_rows4_store_ptrs_unaligned(
                x_bytes[1..].as_ptr(),
                [
                    weight_bytes[3 + row_offsets[0] * 4..].as_ptr(),
                    weight_bytes[3 + row_offsets[1] * 4..].as_ptr(),
                    weight_bytes[3 + row_offsets[2] * 4..].as_ptr(),
                    weight_bytes[3 + row_offsets[3] * 4..].as_ptr(),
                ],
                [
                    out_bytes[1..].as_mut_ptr(),
                    out_bytes[5..].as_mut_ptr(),
                    out_bytes[9..].as_mut_ptr(),
                    out_bytes[13..].as_mut_ptr(),
                ],
                x.len(),
            );
        }

        let got = out_bytes[1..]
            .chunks_exact(4)
            .map(|chunk| f32::from_ne_bytes(chunk.try_into().unwrap()))
            .collect::<Vec<_>>();
        let mut expected = Vec::new();
        for row_off in row_offsets {
            expected.push(
                x.iter()
                    .zip(weights[row_off..row_off + x.len()].iter())
                    .map(|(lhs, rhs)| lhs * rhs)
                    .sum::<f32>(),
            );
        }
        assert_eq!(got.len(), expected.len());
        for (idx, (lhs, rhs)) in got.iter().zip(expected.iter()).enumerate() {
            assert!(
                (lhs - rhs).abs() < 1e-5,
                "idx {idx}: got {lhs}, expected {rhs}"
            );
        }
    }

    #[cfg(any(
        target_arch = "aarch64",
        target_arch = "x86_64",
        all(target_arch = "wasm32", target_feature = "simd128")
    ))]
    #[test]
    fn linear_gate_up4_store_unaligned_matches_reference_outputs() {
        let x = [0.25f32, -1.0, 2.5, 0.5, -0.75, 1.25];
        let weights = [
            1.0f32, 0.0, 0.5, -1.0, 0.25, 1.5, -0.5, 2.0, 0.0, 0.25, 1.0, -1.5, 0.75, -0.5, 1.5,
            0.0, -1.0, 0.5, 1.25, 0.0, -0.75, 2.0, 0.5, -0.25, -1.5, 0.25, 1.0, 0.5, -0.25, 2.0,
            0.0, 1.5, -0.5, 1.25, 0.75, -1.0, 0.5, -1.25, 0.0, 0.75, 1.5, 0.25, 2.0, 0.5, -1.0,
            0.0, -0.5, 1.0,
        ];
        let mut x_bytes = vec![0u8, 0u8];
        x_bytes.extend_from_slice(&f32_bytes(&x));
        let mut weight_bytes = vec![0u8];
        weight_bytes.extend_from_slice(&f32_bytes(&weights));
        let mut out_bytes = vec![0u8; 4 * 4 + 3];

        unsafe {
            linear_gate_up4_store_unaligned(
                x_bytes[2..].as_ptr(),
                0,
                weight_bytes[1..].as_ptr(),
                0,
                x.len(),
                out_bytes[3..].as_mut_ptr(),
            );
        }

        let got = out_bytes[3..]
            .chunks_exact(4)
            .map(|chunk| f32::from_ne_bytes(chunk.try_into().unwrap()))
            .collect::<Vec<_>>();
        let mut expected = Vec::new();
        for hidden_idx in 0..4usize {
            let gate_off = (2 * hidden_idx) * x.len();
            let up_off = (2 * hidden_idx + 1) * x.len();
            let gate = x
                .iter()
                .zip(weights[gate_off..gate_off + x.len()].iter())
                .map(|(lhs, rhs)| lhs * rhs)
                .sum::<f32>();
            let up = x
                .iter()
                .zip(weights[up_off..up_off + x.len()].iter())
                .map(|(lhs, rhs)| lhs * rhs)
                .sum::<f32>();
            let relu = gate.max(0.0);
            expected.push(relu * relu * up);
        }
        assert_eq!(got.len(), expected.len());
        for (idx, (lhs, rhs)) in got.iter().zip(expected.iter()).enumerate() {
            assert!(
                (lhs - rhs).abs() < 1e-5,
                "idx {idx}: got {lhs}, expected {rhs}"
            );
        }
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn linear_dot4_gate_up_interleaved_unaligned_matches_scalar_rows() {
        let x = [0.25f32, -1.0, 2.5, 0.5, -0.75, 1.25];
        let weights = [
            1.0f32, 0.0, 0.5, -1.0, 0.25, 1.5, -0.5, 2.0, 0.0, 0.25, 1.0, -1.5, 0.75, -0.5, 1.5,
            0.0, -1.0, 0.5, 1.25, 0.0, -0.75, 2.0, 0.5, -0.25, -1.5, 0.25, 1.0, 0.5, -0.25, 2.0,
            0.0, 1.5, -0.5, 1.25, 0.75, -1.0, 0.5, -1.25, 0.0, 0.75, 1.5, 0.25, 2.0, 0.5, -1.0,
            0.0, -0.5, 1.0,
        ];
        let mut x_bytes = vec![0u8, 0u8];
        x_bytes.extend_from_slice(&f32_bytes(&x));
        let mut weight_bytes = vec![0u8];
        weight_bytes.extend_from_slice(&f32_bytes(&weights));

        let (gates, ups) = unsafe {
            linear_dot4_gate_up_interleaved_unaligned(
                x_bytes[2..].as_ptr(),
                0,
                weight_bytes[1..].as_ptr(),
                0,
                x.len(),
            )
        };

        for hidden_idx in 0..4usize {
            let gate_off = (2 * hidden_idx) * x.len();
            let up_off = (2 * hidden_idx + 1) * x.len();
            let expected_gate = x
                .iter()
                .zip(weights[gate_off..gate_off + x.len()].iter())
                .map(|(lhs, rhs)| lhs * rhs)
                .sum::<f32>();
            let expected_up = x
                .iter()
                .zip(weights[up_off..up_off + x.len()].iter())
                .map(|(lhs, rhs)| lhs * rhs)
                .sum::<f32>();
            assert!(
                (gates[hidden_idx] - expected_gate).abs() < 1e-5,
                "gate {hidden_idx} mismatch: got {}, expected {expected_gate}",
                gates[hidden_idx]
            );
            assert!(
                (ups[hidden_idx] - expected_up).abs() < 1e-5,
                "up {hidden_idx} mismatch: got {}, expected {expected_up}",
                ups[hidden_idx]
            );
        }
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn linear_dot8_gate_up_interleaved_unaligned_matches_scalar_rows() {
        let x = [0.5f32, -1.0, 1.5, 2.0, -0.25, 0.75];
        let mut weights = Vec::new();
        for hidden_idx in 0..8usize {
            for k in 0..x.len() {
                weights.push((hidden_idx as f32 + 1.0) * (k as f32 - 1.5));
            }
            for k in 0..x.len() {
                weights.push((hidden_idx as f32 + 0.5) * (2.0 - k as f32));
            }
        }
        let mut x_bytes = vec![0u8];
        x_bytes.extend_from_slice(&f32_bytes(&x));
        let mut weight_bytes = vec![0u8, 0u8, 0u8];
        weight_bytes.extend_from_slice(&f32_bytes(&weights));

        let (gates, ups) = unsafe {
            linear_dot8_gate_up_interleaved_unaligned(
                x_bytes[1..].as_ptr(),
                0,
                weight_bytes[3..].as_ptr(),
                0,
                x.len(),
            )
        };

        for hidden_idx in 0..8usize {
            let gate_off = (2 * hidden_idx) * x.len();
            let up_off = (2 * hidden_idx + 1) * x.len();
            let expected_gate = x
                .iter()
                .zip(weights[gate_off..gate_off + x.len()].iter())
                .map(|(lhs, rhs)| lhs * rhs)
                .sum::<f32>();
            let expected_up = x
                .iter()
                .zip(weights[up_off..up_off + x.len()].iter())
                .map(|(lhs, rhs)| lhs * rhs)
                .sum::<f32>();
            assert!(
                (gates[hidden_idx] - expected_gate).abs() < 1e-5,
                "gate {hidden_idx} mismatch: got {}, expected {expected_gate}",
                gates[hidden_idx]
            );
            assert!(
                (ups[hidden_idx] - expected_up).abs() < 1e-5,
                "up {hidden_idx} mismatch: got {}, expected {expected_up}",
                ups[hidden_idx]
            );
        }
    }

    #[cfg(target_arch = "aarch64")]
    #[test]
    fn linear_gate_up8_store_unaligned_matches_reference_outputs() {
        let x = [0.5f32, -1.0, 1.5, 2.0, -0.25, 0.75];
        let mut weights = Vec::new();
        for hidden_idx in 0..8usize {
            for k in 0..x.len() {
                weights.push((hidden_idx as f32 + 1.0) * (k as f32 - 1.5));
            }
            for k in 0..x.len() {
                weights.push((hidden_idx as f32 + 0.5) * (2.0 - k as f32));
            }
        }
        let mut x_bytes = vec![0u8];
        x_bytes.extend_from_slice(&f32_bytes(&x));
        let mut weight_bytes = vec![0u8, 0u8, 0u8];
        weight_bytes.extend_from_slice(&f32_bytes(&weights));
        let mut out_bytes = vec![0u8; 8 * 4 + 3];

        unsafe {
            linear_gate_up8_store_unaligned(
                x_bytes[1..].as_ptr(),
                0,
                weight_bytes[3..].as_ptr(),
                0,
                x.len(),
                out_bytes[3..].as_mut_ptr(),
            );
        }

        let got = out_bytes[3..]
            .chunks_exact(4)
            .map(|chunk| f32::from_ne_bytes(chunk.try_into().unwrap()))
            .collect::<Vec<_>>();
        let mut expected = Vec::new();
        for hidden_idx in 0..8usize {
            let gate_off = (2 * hidden_idx) * x.len();
            let up_off = (2 * hidden_idx + 1) * x.len();
            let gate = x
                .iter()
                .zip(weights[gate_off..gate_off + x.len()].iter())
                .map(|(lhs, rhs)| lhs * rhs)
                .sum::<f32>();
            let up = x
                .iter()
                .zip(weights[up_off..up_off + x.len()].iter())
                .map(|(lhs, rhs)| lhs * rhs)
                .sum::<f32>();
            let relu = gate.max(0.0);
            expected.push(relu * relu * up);
        }
        assert_eq!(got.len(), expected.len());
        for (idx, (lhs, rhs)) in got.iter().zip(expected.iter()).enumerate() {
            assert!(
                (lhs - rhs).abs() < 1e-5,
                "idx {idx}: got {lhs}, expected {rhs}"
            );
        }
    }

    #[test]
    fn gpu_tensor_from_parts_wraps_tensor_and_buffer_objects() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry!(_py, {
            let tensor_name_ptr = alloc_string(_py, b"Tensor");
            let buffer_name_ptr = alloc_string(_py, b"Buffer");
            let tensor_cls_ptr = alloc_class_obj(_py, MoltObject::from_ptr(tensor_name_ptr).bits());
            let buffer_cls_ptr = alloc_class_obj(_py, MoltObject::from_ptr(buffer_name_ptr).bits());
            let data_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 2.0, 3.0, 4.0]));
            let fmt_ptr = alloc_string(_py, b"f");
            let shape_ptr = alloc_tuple(
                _py,
                &[
                    MoltObject::from_int(2).bits(),
                    MoltObject::from_int(2).bits(),
                ],
            );

            let out_bits = molt_gpu_tensor_from_parts(
                MoltObject::from_ptr(tensor_cls_ptr).bits(),
                MoltObject::from_ptr(buffer_cls_ptr).bits(),
                MoltObject::from_ptr(data_ptr).bits(),
                builtin_classes(_py).float,
                MoltObject::from_int(4).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
                MoltObject::from_ptr(shape_ptr).bits(),
                builtin_classes(_py).float,
            );
            assert!(!crate::exception_pending(_py));
            let tensor_ptr = obj_from_bits(out_bits)
                .as_ptr()
                .expect("tensor_from_parts should return a tensor object");

            let buf_name_bits = attr_name_bits_from_bytes(_py, b"_buf").expect("_buf attr");
            let shape_name_bits = attr_name_bits_from_bytes(_py, b"_shape").expect("_shape attr");
            let format_name_bits =
                attr_name_bits_from_bytes(_py, b"_format_char").expect("_format_char attr");
            let itemsize_name_bits =
                attr_name_bits_from_bytes(_py, b"_itemsize").expect("_itemsize attr");

            let buffer_bits = crate::molt_get_attr_name(out_bits, buf_name_bits);
            let shape_bits = crate::molt_get_attr_name(out_bits, shape_name_bits);
            let format_bits = crate::molt_get_attr_name(buffer_bits, format_name_bits);
            let itemsize_bits = crate::molt_get_attr_name(buffer_bits, itemsize_name_bits);

            dec_ref_bits(_py, buf_name_bits);
            dec_ref_bits(_py, shape_name_bits);
            dec_ref_bits(_py, format_name_bits);
            dec_ref_bits(_py, itemsize_name_bits);

            assert_eq!(
                unsafe { crate::object_type_id(tensor_ptr) },
                crate::TYPE_ID_OBJECT
            );
            let shape_ptr = obj_from_bits(shape_bits)
                .as_ptr()
                .expect("tensor shape should be a tuple");
            let dims = unsafe { seq_vec_ref(shape_ptr) };
            assert_eq!(dims.len(), 2);
            assert_eq!(crate::to_i64(obj_from_bits(dims[0])), Some(2));
            assert_eq!(crate::to_i64(obj_from_bits(dims[1])), Some(2));
            assert_eq!(
                crate::string_obj_to_owned(obj_from_bits(format_bits)).as_deref(),
                Some("f")
            );
            assert_eq!(crate::to_i64(obj_from_bits(itemsize_bits)), Some(4));
        });
    }

    #[test]
    fn gpu_repeat_axis_contiguous_f32_roundtrip() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry!(_py, {
            let data_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 2.0, 3.0, 4.0]));
            let fmt_ptr = alloc_string(_py, b"f");
            let shape_ptr = alloc_tuple(
                _py,
                &[
                    MoltObject::from_int(1).bits(),
                    MoltObject::from_int(2).bits(),
                    MoltObject::from_int(2).bits(),
                ],
            );

            let out_bits = molt_gpu_repeat_axis_contiguous(
                MoltObject::from_ptr(data_ptr).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
                MoltObject::from_ptr(shape_ptr).bits(),
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(3).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
            );
            let out_ptr = obj_from_bits(out_bits)
                .as_ptr()
                .expect("repeat intrinsic should return bytes");
            let out =
                unsafe { std::slice::from_raw_parts(bytes_data(out_ptr), bytes_len(out_ptr)) };
            let values = out
                .chunks_exact(4)
                .map(|chunk| f32::from_ne_bytes(chunk.try_into().unwrap()))
                .collect::<Vec<_>>();
            assert_eq!(
                values,
                vec![1.0, 2.0, 1.0, 2.0, 1.0, 2.0, 3.0, 4.0, 3.0, 4.0, 3.0, 4.0]
            );
        });
    }

    #[test]
    fn gpu_buffer_to_list_f32_roundtrip() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry!(_py, {
            let tensor_name_ptr = alloc_string(_py, b"Tensor");
            let buffer_name_ptr = alloc_string(_py, b"Buffer");
            let tensor_cls_ptr = alloc_class_obj(_py, MoltObject::from_ptr(tensor_name_ptr).bits());
            let buffer_cls_ptr = alloc_class_obj(_py, MoltObject::from_ptr(buffer_name_ptr).bits());
            let data_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 2.0, 3.0, 4.0]));
            let fmt_ptr = alloc_string(_py, b"f");
            let shape_ptr = alloc_tuple(_py, &[MoltObject::from_int(4).bits()]);

            let tensor_bits = molt_gpu_tensor_from_parts(
                MoltObject::from_ptr(tensor_cls_ptr).bits(),
                MoltObject::from_ptr(buffer_cls_ptr).bits(),
                MoltObject::from_ptr(data_ptr).bits(),
                builtin_classes(_py).float,
                MoltObject::from_int(4).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
                MoltObject::from_ptr(shape_ptr).bits(),
                builtin_classes(_py).float,
            );
            assert!(!crate::exception_pending(_py));

            let buf_name_bits = attr_name_bits_from_bytes(_py, b"_buf").expect("_buf attr");
            let buffer_bits = crate::molt_get_attr_name(tensor_bits, buf_name_bits);
            dec_ref_bits(_py, buf_name_bits);

            let list_bits = molt_gpu_buffer_to_list(buffer_bits, MoltObject::from_int(4).bits());
            assert!(!crate::exception_pending(_py));
            let list_ptr = obj_from_bits(list_bits)
                .as_ptr()
                .expect("buffer_to_list should return a list");
            let elems = unsafe { seq_vec_ref(list_ptr) };
            let values: Vec<f64> = elems
                .iter()
                .copied()
                .map(|bits| to_f64(obj_from_bits(bits)).expect("float element"))
                .collect();
            assert_eq!(values, vec![1.0, 2.0, 3.0, 4.0]);
        });
    }

    #[test]
    fn gpu_module_tensor_linear_wrapper_roundtrip() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry!(_py, {
            let tensor_name_ptr = alloc_string(_py, b"Tensor");
            let buffer_name_ptr = alloc_string(_py, b"Buffer");
            let tensor_cls_ptr = alloc_class_obj(_py, MoltObject::from_ptr(tensor_name_ptr).bits());
            let buffer_cls_ptr = alloc_class_obj(_py, MoltObject::from_ptr(buffer_name_ptr).bits());
            let tensor_cls_bits = MoltObject::from_ptr(tensor_cls_ptr).bits();
            let buffer_cls_bits = MoltObject::from_ptr(buffer_cls_ptr).bits();

            let x_bits = make_tensor_from_f32(_py, tensor_cls_bits, buffer_cls_bits, &[1.0, 2.0, 3.0, 4.0], &[2, 2]);
            let weight_bits = make_tensor_from_f32(
                _py,
                tensor_cls_bits,
                buffer_cls_bits,
                &[5.0, 6.0, 7.0, 8.0, 9.0, 10.0],
                &[3, 2],
            );

            let out_bits = molt_gpu_tensor__tensor_linear(x_bits, weight_bits);
            assert!(!crate::exception_pending(_py));

            let out_shape_bits = attr_bits(_py, out_bits, b"_shape");
            let out_shape_ptr = obj_from_bits(out_shape_bits).as_ptr().expect("shape tuple");
            let out_dims = unsafe { seq_vec_ref(out_shape_ptr) };
            assert_eq!(crate::to_i64(obj_from_bits(out_dims[0])), Some(2));
            assert_eq!(crate::to_i64(obj_from_bits(out_dims[1])), Some(3));

            let out_buf_bits = attr_bits(_py, out_bits, b"_buf");
            let list_bits = molt_gpu_buffer_to_list(out_buf_bits, MoltObject::from_int(6).bits());
            let list_ptr = obj_from_bits(list_bits).as_ptr().expect("list");
            let elems = unsafe { seq_vec_ref(list_ptr) };
            let values: Vec<f64> = elems
                .iter()
                .copied()
                .map(|bits| to_f64(obj_from_bits(bits)).expect("float element"))
                .collect();
            assert_eq!(values, vec![17.0, 23.0, 29.0, 39.0, 53.0, 67.0]);
        });
    }

    #[test]
    fn gpu_module_tensor_reshape_view_wrapper_reuses_buffer() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry!(_py, {
            let tensor_name_ptr = alloc_string(_py, b"Tensor");
            let buffer_name_ptr = alloc_string(_py, b"Buffer");
            let tensor_cls_ptr = alloc_class_obj(_py, MoltObject::from_ptr(tensor_name_ptr).bits());
            let buffer_cls_ptr = alloc_class_obj(_py, MoltObject::from_ptr(buffer_name_ptr).bits());
            let tensor_cls_bits = MoltObject::from_ptr(tensor_cls_ptr).bits();
            let buffer_cls_bits = MoltObject::from_ptr(buffer_cls_ptr).bits();
            install_gpu_tensor_module(_py, tensor_cls_bits, buffer_cls_bits);

            let tensor_bits = make_tensor_from_f32(
                _py,
                tensor_cls_bits,
                buffer_cls_bits,
                &[1.0, 2.0, 3.0, 4.0],
                &[4],
            );
            let shape_ptr = alloc_tuple(
                _py,
                &[MoltObject::from_int(2).bits(), MoltObject::from_int(2).bits()],
            );

            let reshaped_bits =
                molt_gpu_tensor__tensor_reshape_view(tensor_bits, MoltObject::from_ptr(shape_ptr).bits());
            assert!(!crate::exception_pending(_py));

            let original_buf_bits = attr_bits(_py, tensor_bits, b"_buf");
            let reshaped_buf_bits = attr_bits(_py, reshaped_bits, b"_buf");
            assert_eq!(reshaped_buf_bits, original_buf_bits);

            let reshaped_shape_bits = attr_bits(_py, reshaped_bits, b"_shape");
            let reshaped_shape_ptr = obj_from_bits(reshaped_shape_bits)
                .as_ptr()
                .expect("shape tuple");
            let dims = unsafe { seq_vec_ref(reshaped_shape_ptr) };
            assert_eq!(crate::to_i64(obj_from_bits(dims[0])), Some(2));
            assert_eq!(crate::to_i64(obj_from_bits(dims[1])), Some(2));
        });
    }

    #[test]
    fn gpu_module_tensor_data_list_and_zeros_wrappers_roundtrip() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry!(_py, {
            let tensor_name_ptr = alloc_string(_py, b"Tensor");
            let buffer_name_ptr = alloc_string(_py, b"Buffer");
            let tensor_cls_ptr = alloc_class_obj(_py, MoltObject::from_ptr(tensor_name_ptr).bits());
            let buffer_cls_ptr = alloc_class_obj(_py, MoltObject::from_ptr(buffer_name_ptr).bits());
            let tensor_cls_bits = MoltObject::from_ptr(tensor_cls_ptr).bits();
            let buffer_cls_bits = MoltObject::from_ptr(buffer_cls_ptr).bits();
            install_gpu_tensor_module(_py, tensor_cls_bits, buffer_cls_bits);

            let tensor_bits = make_tensor_from_f32(
                _py,
                tensor_cls_bits,
                buffer_cls_bits,
                &[1.0, 2.0, 3.0, 4.0],
                &[2, 2],
            );
            let list_bits = molt_gpu_tensor__tensor_data_list(tensor_bits);
            assert!(!crate::exception_pending(_py));
            let list_ptr = obj_from_bits(list_bits).as_ptr().expect("list");
            let elems = unsafe { seq_vec_ref(list_ptr) };
            let values: Vec<f64> = elems
                .iter()
                .copied()
                .map(|bits| to_f64(obj_from_bits(bits)).expect("float element"))
                .collect();
            assert_eq!(values, vec![1.0, 2.0, 3.0, 4.0]);

            let zero_shape_ptr = alloc_tuple(
                _py,
                &[MoltObject::from_int(2).bits(), MoltObject::from_int(3).bits()],
            );
            let zeros_bits =
                molt_gpu_tensor__zeros(MoltObject::from_ptr(zero_shape_ptr).bits(), builtin_classes(_py).float);
            assert!(!crate::exception_pending(_py));
            let zero_shape_bits = attr_bits(_py, zeros_bits, b"_shape");
            let zero_shape_ptr = obj_from_bits(zero_shape_bits).as_ptr().expect("shape tuple");
            let zero_dims = unsafe { seq_vec_ref(zero_shape_ptr) };
            assert_eq!(crate::to_i64(obj_from_bits(zero_dims[0])), Some(2));
            assert_eq!(crate::to_i64(obj_from_bits(zero_dims[1])), Some(3));

            let zero_buf_bits = attr_bits(_py, zeros_bits, b"_buf");
            let zero_list_bits = molt_gpu_buffer_to_list(zero_buf_bits, MoltObject::from_int(6).bits());
            let zero_list_ptr = obj_from_bits(zero_list_bits).as_ptr().expect("zero list");
            let zero_values: Vec<f64> = unsafe { seq_vec_ref(zero_list_ptr) }
                .iter()
                .copied()
                .map(|bits| to_f64(obj_from_bits(bits)).expect("float element"))
                .collect();
            assert_eq!(zero_values, vec![0.0; 6]);
        });
    }

    #[test]
    fn gpu_module_tensor_attention_hybrid_mask_wrapper_roundtrip() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry!(_py, {
            let tensor_name_ptr = alloc_string(_py, b"Tensor");
            let buffer_name_ptr = alloc_string(_py, b"Buffer");
            let tensor_cls_ptr = alloc_class_obj(_py, MoltObject::from_ptr(tensor_name_ptr).bits());
            let buffer_cls_ptr = alloc_class_obj(_py, MoltObject::from_ptr(buffer_name_ptr).bits());
            let tensor_cls_bits = MoltObject::from_ptr(tensor_cls_ptr).bits();
            let buffer_cls_bits = MoltObject::from_ptr(buffer_cls_ptr).bits();
            install_gpu_tensor_module(_py, tensor_cls_bits, buffer_cls_bits);

            let token_ids_ptr = alloc_tuple(
                _py,
                &[
                    MoltObject::from_int(17).bits(),
                    MoltObject::from_int(244).bits(),
                    MoltObject::from_int(227).bits(),
                    MoltObject::from_int(230).bits(),
                    MoltObject::from_int(42).bits(),
                ],
            );
            let mask_bits = molt_gpu_tensor__tensor_attention_hybrid_mask(
                MoltObject::from_ptr(token_ids_ptr).bits(),
                MoltObject::from_int(244).bits(),
                MoltObject::from_int(230).bits(),
            );
            assert!(!crate::exception_pending(_py));

            let shape_bits = attr_bits(_py, mask_bits, b"_shape");
            let shape_ptr = obj_from_bits(shape_bits).as_ptr().expect("shape tuple");
            let shape: Vec<i64> = unsafe { seq_vec_ref(shape_ptr) }
                .iter()
                .map(|bits| to_f64(obj_from_bits(*bits)).expect("shape element") as i64)
                .collect();
            assert_eq!(shape, vec![1, 1, 5, 5]);

            let data_list_bits = molt_gpu_tensor__tensor_data_list(mask_bits);
            let data_list_ptr = obj_from_bits(data_list_bits).as_ptr().expect("data list");
            let rows: Vec<f64> = unsafe { seq_vec_ref(data_list_ptr) }
                .iter()
                .copied()
                .map(|bits| to_f64(obj_from_bits(bits)).expect("float element"))
                .collect();
            let at = |q: usize, k: usize| rows[q * 5 + k];
            assert!(at(0, 1) < -1.0e8);
            assert_eq!(at(1, 3), 0.0);
            assert_eq!(at(2, 3), 0.0);
            assert_eq!(at(3, 1), 0.0);
            assert_eq!(at(4, 3), 0.0);
            assert_eq!(at(4, 4), 0.0);
        });
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
            let out =
                unsafe { std::slice::from_raw_parts(bytes_data(out_ptr), bytes_len(out_ptr)) };
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
                &[
                    MoltObject::from_int(2).bits(),
                    MoltObject::from_int(3).bits(),
                ],
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
    fn gpu_linear_split_last_dim_contiguous_f32_three_way_wider_roundtrip() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry!(_py, {
            let x_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 2.0, 3.0]));
            let w_ptr = alloc_bytes(
                _py,
                &f32_bytes(&[
                    1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0, 2.0, 0.0, 1.0, 0.0,
                    2.0, 1.0,
                ]),
            );
            let fmt_ptr = alloc_string(_py, b"f");
            let sizes_ptr = alloc_tuple(
                _py,
                &[
                    MoltObject::from_int(2).bits(),
                    MoltObject::from_int(1).bits(),
                    MoltObject::from_int(3).bits(),
                ],
            );

            let out_bits = molt_gpu_linear_split_last_dim_contiguous(
                MoltObject::from_ptr(x_ptr).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
                MoltObject::from_ptr(w_ptr).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(3).bits(),
                MoltObject::from_ptr(sizes_ptr).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
            );
            let out_ptr = obj_from_bits(out_bits)
                .as_ptr()
                .expect("linear split intrinsic should return tuple");
            let parts = unsafe { crate::seq_vec_ref(out_ptr) };
            assert_eq!(parts.len(), 3);

            let decode = |bits: u64| {
                let ptr = obj_from_bits(bits).as_ptr().expect("bytes");
                let bytes = unsafe { std::slice::from_raw_parts(bytes_data(ptr), bytes_len(ptr)) };
                bytes
                    .chunks_exact(4)
                    .map(|chunk| f32::from_ne_bytes(chunk.try_into().unwrap()))
                    .collect::<Vec<_>>()
            };

            assert_eq!(decode(parts[0]), vec![1.0, 2.0]);
            assert_eq!(decode(parts[1]), vec![3.0]);
            assert_eq!(decode(parts[2]), vec![6.0, 5.0, 7.0]);
        });
    }

    #[test]
    fn gpu_linear_squared_relu_gate_interleaved_contiguous_f32_roundtrip() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry!(_py, {
            let x_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 2.0, 3.0, 4.0]));
            let w_ptr = alloc_bytes(_py, &f32_bytes(&[1.0, 0.0, 0.0, 1.0, 1.0, 1.0, 2.0, 0.0]));
            let fmt_ptr = alloc_string(_py, b"f");

            let out_bits = molt_gpu_linear_squared_relu_gate_interleaved_contiguous(
                MoltObject::from_ptr(x_ptr).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
                MoltObject::from_ptr(w_ptr).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
                MoltObject::from_int(2).bits(),
                MoltObject::from_int(2).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
            );
            let out_ptr = obj_from_bits(out_bits)
                .as_ptr()
                .expect("linear squared relu gate intrinsic should return bytes");
            let out =
                unsafe { std::slice::from_raw_parts(bytes_data(out_ptr), bytes_len(out_ptr)) };
            let mut values = Vec::new();
            for chunk in out.chunks_exact(4) {
                values.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
            }
            assert_eq!(values, vec![2.0, 18.0, 36.0, 294.0]);
        });
    }

    #[test]
    fn gpu_linear_squared_relu_gate_interleaved_contiguous_f32_wide_roundtrip() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry!(_py, {
            let x_ptr = alloc_bytes(_py, &f32_bytes(&[2.0, 3.0]));
            let w_ptr = alloc_bytes(
                _py,
                &f32_bytes(&[
                    1.0, 0.0, 0.0, 1.0, 2.0, 0.0, 0.0, 1.0, 3.0, 0.0, 0.0, 1.0, 4.0, 0.0, 0.0, 1.0,
                    5.0, 0.0, 0.0, 1.0,
                ]),
            );
            let fmt_ptr = alloc_string(_py, b"f");

            let out_bits = molt_gpu_linear_squared_relu_gate_interleaved_contiguous(
                MoltObject::from_ptr(x_ptr).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
                MoltObject::from_ptr(w_ptr).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(2).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
            );
            let out_ptr = obj_from_bits(out_bits)
                .as_ptr()
                .expect("linear squared relu gate intrinsic should return bytes");
            let out =
                unsafe { std::slice::from_raw_parts(bytes_data(out_ptr), bytes_len(out_ptr)) };
            let mut values = Vec::new();
            for chunk in out.chunks_exact(4) {
                values.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
            }
            assert_eq!(values, vec![12.0, 48.0, 108.0, 192.0, 300.0]);
        });
    }

    #[test]
    fn gpu_linear_squared_relu_gate_interleaved_contiguous_f32_wider_roundtrip() {
        let _guard = crate::TEST_MUTEX.lock().unwrap();
        crate::with_gil_entry!(_py, {
            let x_ptr = alloc_bytes(_py, &f32_bytes(&[2.0, 3.0]));
            let w_ptr = alloc_bytes(
                _py,
                &f32_bytes(&[
                    1.0, 0.0, 0.0, 1.0, 2.0, 0.0, 0.0, 1.0, 3.0, 0.0, 0.0, 1.0, 4.0, 0.0, 0.0, 1.0,
                    5.0, 0.0, 0.0, 1.0, 6.0, 0.0, 0.0, 1.0, 7.0, 0.0, 0.0, 1.0, 8.0, 0.0, 0.0, 1.0,
                    9.0, 0.0, 0.0, 1.0,
                ]),
            );
            let fmt_ptr = alloc_string(_py, b"f");

            let out_bits = molt_gpu_linear_squared_relu_gate_interleaved_contiguous(
                MoltObject::from_ptr(x_ptr).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
                MoltObject::from_ptr(w_ptr).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
                MoltObject::from_int(1).bits(),
                MoltObject::from_int(2).bits(),
                MoltObject::from_ptr(fmt_ptr).bits(),
            );
            let out_ptr = obj_from_bits(out_bits)
                .as_ptr()
                .expect("linear squared relu gate intrinsic should return bytes");
            let out =
                unsafe { std::slice::from_raw_parts(bytes_data(out_ptr), bytes_len(out_ptr)) };
            let mut values = Vec::new();
            for chunk in out.chunks_exact(4) {
                values.push(f32::from_ne_bytes(chunk.try_into().unwrap()));
            }
            assert_eq!(
                values,
                vec![12.0, 48.0, 108.0, 192.0, 300.0, 432.0, 588.0, 768.0, 972.0]
            );
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
                &[
                    MoltObject::from_int(2).bits(),
                    MoltObject::from_int(2).bits(),
                ],
            );
            let b_shape_ptr = alloc_tuple(
                _py,
                &[
                    MoltObject::from_int(1).bits(),
                    MoltObject::from_int(2).bits(),
                ],
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
            let out =
                unsafe { std::slice::from_raw_parts(bytes_data(out_ptr), bytes_len(out_ptr)) };
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
                &[
                    MoltObject::from_int(2).bits(),
                    MoltObject::from_int(2).bits(),
                ],
            );
            let b_shape_ptr = alloc_tuple(
                _py,
                &[
                    MoltObject::from_int(2).bits(),
                    MoltObject::from_int(2).bits(),
                ],
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
            let out =
                unsafe { std::slice::from_raw_parts(bytes_data(out_ptr), bytes_len(out_ptr)) };
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
            let _ = crate::molt_exception_clear();
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
                &[
                    MoltObject::from_int(2).bits(),
                    MoltObject::from_int(2).bits(),
                ],
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
                &[
                    MoltObject::from_int(2).bits(),
                    MoltObject::from_int(2).bits(),
                ],
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
            let x_ptr = alloc_bytes(
                _py,
                &f32_bytes(&[1.0, 10.0, -2.0, 20.0, 3.0, 30.0, 4.0, 40.0]),
            );
            let fmt_ptr = alloc_string(_py, b"f");
            let shape_ptr = alloc_tuple(
                _py,
                &[
                    MoltObject::from_int(1).bits(),
                    MoltObject::from_int(8).bits(),
                ],
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
