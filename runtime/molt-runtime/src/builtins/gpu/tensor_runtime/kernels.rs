use super::*;

pub(super) fn parse_shape(
    _py: &crate::PyToken<'_>,
    bits: u64,
    role: &str,
) -> Result<Vec<usize>, u64> {
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

pub(super) fn product(shape: &[usize]) -> usize {
    let mut out = 1usize;
    for dim in shape {
        out *= *dim;
    }
    out
}

pub(super) fn strides(shape: &[usize]) -> Vec<usize> {
    let mut out = vec![0; shape.len()];
    let mut stride = 1usize;
    for (i, dim) in shape.iter().enumerate().rev() {
        out[i] = stride;
        stride *= *dim;
    }
    out
}

pub(super) fn validate_permutation(
    _py: &crate::PyToken<'_>,
    dims: &[usize],
    ndim: usize,
) -> Result<(), u64> {
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

pub(super) fn apply_binary_op(
    _py: &crate::PyToken<'_>,
    op_code: i64,
    a: f64,
    b: f64,
) -> Result<f64, u64> {
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

pub(super) unsafe fn read_scalar(ptr: *const u8, index: usize, fmt: ScalarFormat) -> f64 {
    match fmt {
        ScalarFormat::F32 => unsafe { (ptr.add(index * 4) as *const f32).read_unaligned() as f64 },
        ScalarFormat::F64 => unsafe { (ptr.add(index * 8) as *const f64).read_unaligned() },
        ScalarFormat::I64 => unsafe { (ptr.add(index * 8) as *const i64).read_unaligned() as f64 },
    }
}

pub(super) unsafe fn write_scalar(ptr: *mut u8, index: usize, fmt: ScalarFormat, value: f64) {
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
pub(super) unsafe fn aligned_f32_slice<'a>(ptr: *const u8, len: usize) -> Option<&'a [f32]> {
    if !(ptr as usize).is_multiple_of(std::mem::align_of::<f32>()) {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts(ptr as *const f32, len) })
}

#[inline]
pub(super) unsafe fn aligned_f32_slice_mut<'a>(ptr: *mut u8, len: usize) -> Option<&'a mut [f32]> {
    if !(ptr as usize).is_multiple_of(std::mem::align_of::<f32>()) {
        return None;
    }
    Some(unsafe { std::slice::from_raw_parts_mut(ptr as *mut f32, len) })
}

#[cfg(all(target_arch = "aarch64", not(miri)))]
#[inline]
pub(super) unsafe fn load_f32x4_bytes_unaligned(
    ptr: *const u8,
) -> core::arch::aarch64::float32x4_t {
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

#[cfg(all(target_arch = "aarch64", not(miri)))]
#[inline]
pub(super) unsafe fn load_f32_bytes_unaligned(ptr: *const u8) -> f32 {
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

#[cfg(all(target_arch = "aarch64", not(miri)))]
#[inline]
pub(super) unsafe fn store_f32_bytes_unaligned(ptr: *mut u8, value: f32) {
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

#[cfg(all(target_arch = "aarch64", not(miri)))]
#[inline]
pub(super) unsafe fn linear_dot_ptrs_unaligned(
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

#[cfg(all(target_arch = "aarch64", not(miri)))]
#[inline]
pub(super) unsafe fn linear_dot4_unaligned(
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
pub(super) unsafe fn horizontal_sum_f32x4(acc: std::arch::x86_64::__m128) -> f32 {
    use std::arch::x86_64::*;
    let hi = unsafe { _mm_movehl_ps(acc, acc) };
    let sum2 = unsafe { _mm_add_ps(acc, hi) };
    let shuffled = unsafe { _mm_shuffle_ps(sum2, sum2, 0x55) };
    let sum1 = unsafe { _mm_add_ss(sum2, shuffled) };
    unsafe { _mm_cvtss_f32(sum1) }
}

#[cfg(target_arch = "x86_64")]
#[inline]
pub(super) unsafe fn linear_dot_ptrs_unaligned(
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
pub(super) unsafe fn linear_dot4_unaligned(
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
pub(super) unsafe fn horizontal_sum_f32x4(acc: std::arch::wasm32::v128) -> f32 {
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
pub(super) unsafe fn linear_dot_ptrs_unaligned(
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
pub(super) unsafe fn linear_dot4_unaligned(
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

#[cfg(all(target_arch = "aarch64", not(miri), test))]
#[inline]
pub(super) unsafe fn linear_dot4_rows_ptrs_unaligned(
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

#[cfg(all(target_arch = "aarch64", not(miri)))]
#[inline]
pub(super) unsafe fn linear_rows4_store_ptrs_unaligned(
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
pub(super) unsafe fn linear_rows4_store_ptrs_unaligned(
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
pub(super) unsafe fn linear_rows4_store_ptrs_unaligned(
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

#[cfg(all(target_arch = "aarch64", not(miri), test))]
#[inline]
pub(super) unsafe fn linear_dot4_rows_unaligned(
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

#[cfg(all(target_arch = "aarch64", not(miri), test))]
#[inline]
pub(super) unsafe fn linear_dot4_gate_up_interleaved_ptrs_unaligned(
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

#[cfg(all(target_arch = "aarch64", not(miri)))]
#[inline]
pub(super) unsafe fn linear_gate_up4_store_ptrs_unaligned(
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
pub(super) unsafe fn linear_gate_up4_store_ptrs_unaligned(
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
pub(super) unsafe fn linear_gate_up4_store_ptrs_unaligned(
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
        all(target_arch = "aarch64", not(miri)),
        target_arch = "x86_64",
        all(target_arch = "wasm32", target_feature = "simd128")
    ),
    test
))]
#[inline]
pub(super) unsafe fn linear_gate_up4_store_unaligned(
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

#[cfg(all(target_arch = "aarch64", not(miri), test))]
#[inline]
pub(super) unsafe fn linear_dot4_gate_up_interleaved_unaligned(
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

#[cfg(all(target_arch = "aarch64", not(miri)))]
#[cfg(all(target_arch = "aarch64", not(miri)))]
#[cfg(all(target_arch = "aarch64", not(miri)))]
pub(super) unsafe fn linear_gate_up8_store_group_unaligned(
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

#[cfg(all(target_arch = "aarch64", not(miri), test))]
#[inline]
pub(super) unsafe fn linear_gate_up8_store_unaligned(
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

#[cfg(all(target_arch = "aarch64", not(miri), test))]
#[inline]
pub(super) unsafe fn linear_dot8_gate_up_interleaved_ptrs_unaligned(
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

#[cfg(all(target_arch = "aarch64", not(miri), test))]
#[inline]
pub(super) unsafe fn linear_dot8_gate_up_interleaved_unaligned(
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

pub(super) unsafe fn linear_rows_f32(
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
    }

    #[cfg(any(
        all(target_arch = "aarch64", not(miri)),
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
    }

    #[cfg(not(any(
        all(target_arch = "aarch64", not(miri)),
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
    all(target_arch = "aarch64", not(miri)),
    target_arch = "x86_64",
    all(target_arch = "wasm32", target_feature = "simd128")
))]
pub(super) unsafe fn linear_split_last_dim_f32(
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
    all(target_arch = "aarch64", not(miri)),
    target_arch = "x86_64",
    all(target_arch = "wasm32", target_feature = "simd128")
)))]
pub(super) unsafe fn linear_split_last_dim_f32(
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

pub(super) unsafe fn linear_squared_relu_gate_interleaved_f32(
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
    }

    #[cfg(all(target_arch = "aarch64", not(miri)))]
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
    }

    #[cfg(not(any(
        all(target_arch = "aarch64", not(miri)),
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

pub(super) unsafe fn matmul_f32(
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

pub(super) unsafe fn rope_apply_f32(
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

pub(super) unsafe fn softmax_last_axis_f32(
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

pub(super) unsafe fn rms_norm_last_axis_f32(
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

pub(super) unsafe fn squared_relu_gate_interleaved_f32(
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
