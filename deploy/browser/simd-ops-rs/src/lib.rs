//! WASM SIMD vectorized ops for Falcon-OCR inference.
//!
//! Rust source for the optimized SIMD operations. Compiles to wasm32-unknown-unknown
//! with SIMD enabled.
//!
//! Build (`.cargo/config.toml` enables simd128):
//!   cargo build --target wasm32-unknown-unknown --release
//!
//! Kernels:
//! - Matmul: 4x4 tiled with SIMD f32x4 for cache locality
//! - Softmax: SIMD maximum, fused exp/store/double sum, SIMD normalization
//! - exp2: Degree-four polynomial with explicit scalar and SIMD evaluation order

#![no_std]

use core::arch::wasm32::*;

// ---------------------------------------------------------------------------
// Scalar math ops via WASM SIMD (f32 methods unavailable in no_std wasm32)
// ---------------------------------------------------------------------------
#[inline(always)]
fn sqrt_scalar(x: f32) -> f32 {
    let v = f32x4_splat(x);
    f32x4_extract_lane::<0>(f32x4_sqrt(v))
}

#[inline(always)]
fn floor_scalar(x: f32) -> f32 {
    let v = f32x4_splat(x);
    f32x4_extract_lane::<0>(f32x4_floor(v))
}

/// WebAssembly maximum semantics, with a deterministic quiet NaN.
#[inline(always)]
fn max_scalar(a: f32, b: f32) -> f32 {
    if a.is_nan() || b.is_nan() {
        f32::from_bits(0x7fc0_0000)
    } else if a == b {
        // Equal nonzero values have identical bits; mixed zeros select +0.
        f32::from_bits(a.to_bits() & b.to_bits())
    } else if a > b {
        a
    } else {
        b
    }
}

#[inline(always)]
fn max_v128(a: v128, b: v128) -> v128 {
    let nan_mask = v128_or(f32x4_ne(a, a), f32x4_ne(b, b));
    v128_bitselect(i32x4_splat(0x7fc0_0000), f32x4_max(a, b), nan_mask)
}

// ---------------------------------------------------------------------------
// exp2 polynomial — positive degree-four interpolant on [0, 1].
//
// Chebyshev-Lobatto nodes: 0, (2-sqrt(2))/4, 1/2, (2+sqrt(2))/4, 1.
// C1..C3 are the interpolant rounded to f32; C4 is its endpoint residual.
// Positive coefficients and exact P(1)=2 retain monotonicity in f32 Horner
// evaluation. See docs/design/browser_simd.md for derivation/error policy.
// This is not a correctly rounded exp2 implementation.
// ---------------------------------------------------------------------------
const EXP2_C0: f32 = 1.0;
const EXP2_C1: f32 = 0.693_035_3;
const EXP2_C2: f32 = 0.241_444_51;
const EXP2_C3: f32 = 0.051_836_18;
// f64 represents this sum of f32 coefficients exactly. Compile-time only.
const EXP2_C4: f32 =
    (2.0 - (EXP2_C0 as f64 + EXP2_C1 as f64 + EXP2_C2 as f64 + EXP2_C3 as f64)) as f32;

/// Scalar exp2 approximation via a monotone degree-four polynomial.
#[inline(always)]
fn exp2_scalar(x: f32) -> f32 {
    if x.abs() < 126.0 {
        exp2_normal_scalar(x)
    } else {
        exp2_special_scalar(x)
    }
}

// Keep rare-domain classification out of every normal SIMD lane and softmax step.
#[cold]
#[inline(never)]
fn exp2_special_scalar(x: f32) -> f32 {
    if x.is_nan() {
        return f32::from_bits(0x7fc0_0000);
    }
    if x >= 128.0 {
        return f32::INFINITY;
    }
    if x <= -150.0 {
        return 0.0;
    }
    if x < -126.0 {
        // Keep exponent injection normal, then round into the subnormal range.
        return exp2_normal_scalar(x + 24.0) * f32::from_bits(103 << 23);
    }
    exp2_normal_scalar(x)
}

/// The positive-coefficient polynomial and exponent injection for finite [-126, 128).
#[inline(always)]
fn exp2_normal_scalar(x: f32) -> f32 {
    let xi = floor_scalar(x);
    let xf = x - xi;
    // Positive argument/coefficient Horner evaluation is monotone in f32.
    let mut p = EXP2_C4;
    p = p * xf + EXP2_C3;
    p = p * xf + EXP2_C2;
    p = p * xf + EXP2_C1;
    p = p * xf + EXP2_C0;
    // 2^xi via IEEE 754 exponent manipulation
    let exp_bits = ((xi as i32) + 127) << 23;
    p * f32::from_bits(exp_bits as u32)
}

/// SIMD v128 exp2 — 4 lanes, monotone degree-four polynomial.
#[inline(always)]
fn exp2_v128(x: v128) -> v128 {
    if !i32x4_all_true(f32x4_lt(f32x4_abs(x), f32x4_splat(126.0))) {
        return exp2_special_v128(x);
    }
    let xi = f32x4_floor(x);
    let xf = f32x4_sub(x, xi);

    // The same positive-coefficient Horner order as the scalar lane.
    let mut p = f32x4_splat(EXP2_C4);
    p = f32x4_add(f32x4_mul(p, xf), f32x4_splat(EXP2_C3));
    p = f32x4_add(f32x4_mul(p, xf), f32x4_splat(EXP2_C2));
    p = f32x4_add(f32x4_mul(p, xf), f32x4_splat(EXP2_C1));
    p = f32x4_add(f32x4_mul(p, xf), f32x4_splat(EXP2_C0));

    // 2^xi via IEEE 754: bits = (trunc(xi) + 127) << 23
    let xi_i = i32x4_trunc_sat_f32x4(xi);
    let exp_bits = i32x4_shl(i32x4_add(xi_i, i32x4_splat(127)), 23);
    f32x4_mul(p, exp_bits) // reinterpret i32x4 bits as f32x4 (same v128 type)
}

#[cold]
#[inline(never)]
fn exp2_special_v128(x: v128) -> v128 {
    f32x4(
        exp2_special_scalar(f32x4_extract_lane::<0>(x)),
        exp2_special_scalar(f32x4_extract_lane::<1>(x)),
        exp2_special_scalar(f32x4_extract_lane::<2>(x)),
        exp2_special_scalar(f32x4_extract_lane::<3>(x)),
    )
}

/// Horizontal sum of 4 f32 lanes using pairwise shuffle reduction.
///
/// Stays in v128 for 2 rounds of shuffle+add before a single lane extract,
/// reducing 4 lane extractions to 1 and exploiting SIMD pipeline throughput.
///   Round 1: [a,b,c,d] + [c,d,a,b] = [a+c, b+d, c+a, d+b]
///   Round 2: [a+c, b+d, ...] + [b+d, a+c, ...] = [(a+c)+(b+d), ...]
#[inline(always)]
fn hsum_f32x4(v: v128) -> f32 {
    // Shuffle high pair to low: [c, d, a, b]
    let hi = i32x4_shuffle::<2, 3, 0, 1>(v, v);
    let sum1 = f32x4_add(v, hi); // [a+c, b+d, ...]
    // Shuffle odd to even: [b+d, a+c, ...]
    let odd = i32x4_shuffle::<1, 0, 3, 2>(sum1, sum1);
    let sum2 = f32x4_add(sum1, odd); // [(a+c)+(b+d), ...]
    f32x4_extract_lane::<0>(sum2)
}

/// Horizontal max of 4 f32 lanes using pairwise shuffle reduction.
#[inline(always)]
fn hmax_f32x4(v: v128) -> f32 {
    let hi = i32x4_shuffle::<2, 3, 0, 1>(v, v);
    let max1 = max_v128(v, hi);
    let odd = i32x4_shuffle::<1, 0, 3, 2>(max1, max1);
    let max2 = max_v128(max1, odd);
    f32x4_extract_lane::<0>(max2)
}

// ---------------------------------------------------------------------------
// Tiled matmul — 4x4 register tiling with SIMD f32x4.
//
// For each 4x4 output tile, we load 4 rows of A and 4 columns of B,
// accumulating 4x4 = 16 dot-product results in registers. This gives
// 4x better data reuse vs the IKJ loop in the WAT version.
//
// Memory access pattern:
//   - A is accessed row-by-row (sequential, cache-friendly)
//   - B is accessed by 4-wide column strips via f32x4 loads
//   - Output C is written in 4-wide strips
// ---------------------------------------------------------------------------

/// Tiled matmul: C = A @ B where A is [M, K] and B is [K, N].
///
/// Uses 4x4 register tiling with K-loop unrolled by 4: processes 4 rows of A
/// and 4 columns of B simultaneously with four f32x4 accumulators.
/// Inner K loop processes 4 values per iteration for reduced loop overhead
/// and better instruction-level parallelism.
///
/// # Safety
///
/// Each pointer must be non-null, aligned for `f32`, and valid for the reads
/// and writes that the element counts above imply.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn matmul_f32_tiled(
    a: *const f32,
    b: *const f32,
    out: *mut f32,
    m: u32,
    k: u32,
    n: u32,
) {
    // SAFETY: the caller passes pointers valid for the element counts this
    // export documents, as its C ABI contract states.
    unsafe {
        let m = m as usize;
        let k = k as usize;
        let n = n as usize;

        // Zero output
        let out_bytes = m * n * 4;
        core::ptr::write_bytes(out as *mut u8, 0, out_bytes);

        let n4 = n & !3; // n rounded down to multiple of 4
        let k4 = k & !3; // k rounded down to multiple of 4

        // Process 4 rows at a time
        let m4 = m & !3;
        let mut mi = 0usize;

        while mi < m4 {
            // Base pointers for 4 rows of A
            let a_row0 = a.add(mi * k);
            let a_row1 = a.add((mi + 1) * k);
            let a_row2 = a.add((mi + 2) * k);
            let a_row3 = a.add((mi + 3) * k);

            // For each 4-wide column strip of the output
            let mut ni = 0usize;
            while ni < n4 {
                // 4 accumulators: one per row, each holding 4 columns
                let mut acc0 = f32x4_splat(0.0);
                let mut acc1 = f32x4_splat(0.0);
                let mut acc2 = f32x4_splat(0.0);
                let mut acc3 = f32x4_splat(0.0);

                // Unrolled K loop: process 4 K values per iteration
                let mut ki = 0usize;
                while ki < k4 {
                    // Iteration 0
                    let b_vec0 = v128_load(b.add(ki * n + ni) as *const v128);
                    let a00 = f32x4_splat(*a_row0.add(ki));
                    let a10 = f32x4_splat(*a_row1.add(ki));
                    let a20 = f32x4_splat(*a_row2.add(ki));
                    let a30 = f32x4_splat(*a_row3.add(ki));
                    acc0 = f32x4_add(acc0, f32x4_mul(a00, b_vec0));
                    acc1 = f32x4_add(acc1, f32x4_mul(a10, b_vec0));
                    acc2 = f32x4_add(acc2, f32x4_mul(a20, b_vec0));
                    acc3 = f32x4_add(acc3, f32x4_mul(a30, b_vec0));

                    // Iteration 1
                    let b_vec1 = v128_load(b.add((ki + 1) * n + ni) as *const v128);
                    let a01 = f32x4_splat(*a_row0.add(ki + 1));
                    let a11 = f32x4_splat(*a_row1.add(ki + 1));
                    let a21 = f32x4_splat(*a_row2.add(ki + 1));
                    let a31 = f32x4_splat(*a_row3.add(ki + 1));
                    acc0 = f32x4_add(acc0, f32x4_mul(a01, b_vec1));
                    acc1 = f32x4_add(acc1, f32x4_mul(a11, b_vec1));
                    acc2 = f32x4_add(acc2, f32x4_mul(a21, b_vec1));
                    acc3 = f32x4_add(acc3, f32x4_mul(a31, b_vec1));

                    // Iteration 2
                    let b_vec2 = v128_load(b.add((ki + 2) * n + ni) as *const v128);
                    let a02 = f32x4_splat(*a_row0.add(ki + 2));
                    let a12 = f32x4_splat(*a_row1.add(ki + 2));
                    let a22 = f32x4_splat(*a_row2.add(ki + 2));
                    let a32 = f32x4_splat(*a_row3.add(ki + 2));
                    acc0 = f32x4_add(acc0, f32x4_mul(a02, b_vec2));
                    acc1 = f32x4_add(acc1, f32x4_mul(a12, b_vec2));
                    acc2 = f32x4_add(acc2, f32x4_mul(a22, b_vec2));
                    acc3 = f32x4_add(acc3, f32x4_mul(a32, b_vec2));

                    // Iteration 3
                    let b_vec3 = v128_load(b.add((ki + 3) * n + ni) as *const v128);
                    let a03 = f32x4_splat(*a_row0.add(ki + 3));
                    let a13 = f32x4_splat(*a_row1.add(ki + 3));
                    let a23 = f32x4_splat(*a_row2.add(ki + 3));
                    let a33 = f32x4_splat(*a_row3.add(ki + 3));
                    acc0 = f32x4_add(acc0, f32x4_mul(a03, b_vec3));
                    acc1 = f32x4_add(acc1, f32x4_mul(a13, b_vec3));
                    acc2 = f32x4_add(acc2, f32x4_mul(a23, b_vec3));
                    acc3 = f32x4_add(acc3, f32x4_mul(a33, b_vec3));

                    ki += 4;
                }

                // K tail: remaining 0-3 elements
                while ki < k {
                    let b_vec = v128_load(b.add(ki * n + ni) as *const v128);
                    let a0 = f32x4_splat(*a_row0.add(ki));
                    let a1 = f32x4_splat(*a_row1.add(ki));
                    let a2 = f32x4_splat(*a_row2.add(ki));
                    let a3 = f32x4_splat(*a_row3.add(ki));
                    acc0 = f32x4_add(acc0, f32x4_mul(a0, b_vec));
                    acc1 = f32x4_add(acc1, f32x4_mul(a1, b_vec));
                    acc2 = f32x4_add(acc2, f32x4_mul(a2, b_vec));
                    acc3 = f32x4_add(acc3, f32x4_mul(a3, b_vec));
                    ki += 1;
                }

                // Store 4x4 tile to output
                v128_store(out.add(mi * n + ni) as *mut v128, acc0);
                v128_store(out.add((mi + 1) * n + ni) as *mut v128, acc1);
                v128_store(out.add((mi + 2) * n + ni) as *mut v128, acc2);
                v128_store(out.add((mi + 3) * n + ni) as *mut v128, acc3);

                ni += 4;
            }

            // Scalar tail for remaining columns
            for row in mi..mi + 4 {
                for col in n4..n {
                    let mut sum = 0.0f32;
                    for ki in 0..k {
                        sum += *a.add(row * k + ki) * *b.add(ki * n + col);
                    }
                    *out.add(row * n + col) = sum;
                }
            }

            mi += 4;
        }

        // Scalar tail for remaining rows
        for row in m4..m {
            let mut ni = 0usize;
            while ni < n4 {
                let mut acc = f32x4_splat(0.0);
                for ki in 0..k {
                    let a_val = f32x4_splat(*a.add(row * k + ki));
                    let b_vec = v128_load(b.add(ki * n + ni) as *const v128);
                    acc = f32x4_add(acc, f32x4_mul(a_val, b_vec));
                }
                v128_store(out.add(row * n + ni) as *mut v128, acc);
                ni += 4;
            }
            for col in n4..n {
                let mut sum = 0.0f32;
                for ki in 0..k {
                    sum += *a.add(row * k + ki) * *b.add(ki * n + col);
                }
                *out.add(row * n + col) = sum;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Stable softmax: SIMD max, fused exp/store/sum, SIMD normalization.
//
// Computing exponents once avoids repeated approximation during online
// rescaling. f64 SIMD accumulation bounds positive-sum error for long rows.
// The output buffer stores the unnormalized exponents; no scratch allocation.
// ---------------------------------------------------------------------------

/// Stable softmax with one exponent evaluation per element.
///
/// # Safety
///
/// Each pointer must be non-null, aligned for `f32`, and valid for the reads
/// and writes that the element counts above imply.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn softmax_f32_fused(a: *const f32, out: *mut f32, n: u32) {
    // SAFETY: both ranges contain n f32 elements as the caller promises.
    unsafe {
        if n == 0 {
            return;
        }
        let max_val = reduce_max_f32(a, n);
        let n = n as usize;
        let n4 = n & !3;
        let max_splat = f32x4_splat(max_val);
        let log2e_splat = f32x4_splat(core::f32::consts::LOG2_E);
        let mut sum_low = f64x2_splat(0.0);
        let mut sum_high = f64x2_splat(0.0);
        let mut i = 0usize;
        while i < n4 {
            let x = v128_load(a.add(i) as *const v128);
            let shifted = f32x4_mul(f32x4_sub(x, max_splat), log2e_splat);
            let exp_val = exp2_v128(shifted);
            v128_store(out.add(i) as *mut v128, exp_val);
            sum_low = f64x2_add(sum_low, f64x2_promote_low_f32x4(exp_val));
            let high = i32x4_shuffle::<2, 3, 2, 3>(exp_val, exp_val);
            sum_high = f64x2_add(sum_high, f64x2_promote_low_f32x4(high));
            i += 4;
        }
        let sum_lanes = f64x2_add(sum_low, sum_high);
        let mut sum = f64x2_extract_lane::<0>(sum_lanes) + f64x2_extract_lane::<1>(sum_lanes);
        while i < n {
            let value = exp2_scalar((*a.add(i) - max_val) * core::f32::consts::LOG2_E);
            *out.add(i) = value;
            sum += value as f64;
            i += 1;
        }
        let inv_sum = (1.0 / sum) as f32;
        let inv_splat = f32x4_splat(inv_sum);
        i = 0;
        while i < n4 {
            let value = v128_load(out.add(i) as *const v128);
            v128_store(out.add(i) as *mut v128, f32x4_mul(value, inv_splat));
            i += 4;
        }
        while i < n {
            *out.add(i) *= inv_sum;
            i += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// exp2 — vectorized, endpoint-constrained degree-four approximation.
// ---------------------------------------------------------------------------

/// exp2(x) for n elements, monotone degree-four approximation.
///
/// # Safety
///
/// Each pointer must be non-null, aligned for `f32`, and valid for the reads
/// and writes that the element counts above imply.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn exp2_f32(a: *const f32, out: *mut f32, n: u32) {
    // SAFETY: the caller passes pointers valid for the element counts this
    // export documents, as its C ABI contract states.
    unsafe {
        let n = n as usize;
        let n4 = n & !3;

        let mut i = 0usize;
        while i < n4 {
            let x = v128_load(a.add(i) as *const v128);
            let result = exp2_v128(x);
            v128_store(out.add(i) as *mut v128, result);
            i += 4;
        }

        while i < n {
            *out.add(i) = exp2_scalar(*a.add(i));
            i += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Elementwise ops: add, mul, neg, sqrt, reciprocal, max.
// ---------------------------------------------------------------------------

///
/// # Safety
///
/// Each pointer must be non-null, aligned for `f32`, and valid for the reads
/// and writes that the element counts above imply.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn add_f32(a: *const f32, b: *const f32, out: *mut f32, n: u32) {
    // SAFETY: the caller passes pointers valid for the element counts this
    // export documents, as its C ABI contract states.
    unsafe {
        let n = n as usize;
        let n4 = n & !3;
        let mut i = 0usize;
        while i < n4 {
            let va = v128_load(a.add(i) as *const v128);
            let vb = v128_load(b.add(i) as *const v128);
            v128_store(out.add(i) as *mut v128, f32x4_add(va, vb));
            i += 4;
        }
        while i < n {
            *out.add(i) = *a.add(i) + *b.add(i);
            i += 1;
        }
    }
}

///
/// # Safety
///
/// Each pointer must be non-null, aligned for `f32`, and valid for the reads
/// and writes that the element counts above imply.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mul_f32(a: *const f32, b: *const f32, out: *mut f32, n: u32) {
    // SAFETY: the caller passes pointers valid for the element counts this
    // export documents, as its C ABI contract states.
    unsafe {
        let n = n as usize;
        let n4 = n & !3;
        let mut i = 0usize;
        while i < n4 {
            let va = v128_load(a.add(i) as *const v128);
            let vb = v128_load(b.add(i) as *const v128);
            v128_store(out.add(i) as *mut v128, f32x4_mul(va, vb));
            i += 4;
        }
        while i < n {
            *out.add(i) = *a.add(i) * *b.add(i);
            i += 1;
        }
    }
}

///
/// # Safety
///
/// Each pointer must be non-null, aligned for `f32`, and valid for the reads
/// and writes that the element counts above imply.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn neg_f32(a: *const f32, out: *mut f32, n: u32) {
    // SAFETY: the caller passes pointers valid for the element counts this
    // export documents, as its C ABI contract states.
    unsafe {
        let n = n as usize;
        let n4 = n & !3;
        let mut i = 0usize;
        while i < n4 {
            let va = v128_load(a.add(i) as *const v128);
            v128_store(out.add(i) as *mut v128, f32x4_neg(va));
            i += 4;
        }
        while i < n {
            *out.add(i) = -*a.add(i);
            i += 1;
        }
    }
}

///
/// # Safety
///
/// Each pointer must be non-null, aligned for `f32`, and valid for the reads
/// and writes that the element counts above imply.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn sqrt_f32(a: *const f32, out: *mut f32, n: u32) {
    // SAFETY: the caller passes pointers valid for the element counts this
    // export documents, as its C ABI contract states.
    unsafe {
        let n = n as usize;
        let n4 = n & !3;
        let mut i = 0usize;
        while i < n4 {
            let va = v128_load(a.add(i) as *const v128);
            v128_store(out.add(i) as *mut v128, f32x4_sqrt(va));
            i += 4;
        }
        while i < n {
            *out.add(i) = sqrt_scalar(*a.add(i));
            i += 1;
        }
    }
}

///
/// # Safety
///
/// Each pointer must be non-null, aligned for `f32`, and valid for the reads
/// and writes that the element counts above imply.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn reciprocal_f32(a: *const f32, out: *mut f32, n: u32) {
    // SAFETY: the caller passes pointers valid for the element counts this
    // export documents, as its C ABI contract states.
    unsafe {
        let n = n as usize;
        let n4 = n & !3;
        let ones = f32x4_splat(1.0);
        let mut i = 0usize;
        while i < n4 {
            let va = v128_load(a.add(i) as *const v128);
            v128_store(out.add(i) as *mut v128, f32x4_div(ones, va));
            i += 4;
        }
        while i < n {
            *out.add(i) = 1.0 / *a.add(i);
            i += 1;
        }
    }
}

/// Max with NaN propagation: if either operand is NaN, output is NaN.
///
/// # Safety
///
/// Each pointer must be non-null, aligned for `f32`, and valid for the reads
/// and writes that the element counts above imply.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn max_f32(a: *const f32, b: *const f32, out: *mut f32, n: u32) {
    // SAFETY: the caller passes pointers valid for the element counts this
    // export documents, as its C ABI contract states.
    unsafe {
        let n = n as usize;
        let n4 = n & !3;
        let mut i = 0usize;
        while i < n4 {
            let va = v128_load(a.add(i) as *const v128);
            let vb = v128_load(b.add(i) as *const v128);
            let result = max_v128(va, vb);
            v128_store(out.add(i) as *mut v128, result);
            i += 4;
        }
        while i < n {
            let av = *a.add(i);
            let bv = *b.add(i);
            *out.add(i) = max_scalar(av, bv);
            i += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Reductions: sum, max.
// ---------------------------------------------------------------------------

///
/// # Safety
///
/// Each pointer must be non-null, aligned for `f32`, and valid for the reads
/// and writes that the element counts above imply.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn reduce_sum_f32(a: *const f32, n: u32) -> f32 {
    // SAFETY: the caller passes pointers valid for the element counts this
    // export documents, as its C ABI contract states.
    unsafe {
        let n = n as usize;
        let n4 = n & !3;
        let mut acc = f32x4_splat(0.0);
        let mut i = 0usize;
        while i < n4 {
            acc = f32x4_add(acc, v128_load(a.add(i) as *const v128));
            i += 4;
        }
        let mut sum = hsum_f32x4(acc);
        while i < n {
            sum += *a.add(i);
            i += 1;
        }
        sum
    }
}

///
/// # Safety
///
/// Each pointer must be non-null, aligned for `f32`, and valid for the reads
/// and writes that the element counts above imply.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn reduce_max_f32(a: *const f32, n: u32) -> f32 {
    // SAFETY: the caller passes pointers valid for the element counts this
    // export documents, as its C ABI contract states.
    unsafe {
        let n = n as usize;
        let n4 = n & !3;
        let mut acc = f32x4_splat(f32::NEG_INFINITY);
        let mut i = 0usize;
        while i < n4 {
            let v = v128_load(a.add(i) as *const v128);
            acc = max_v128(acc, v);
            i += 4;
        }
        let mut maxval = hmax_f32x4(acc);
        while i < n {
            let v = *a.add(i);
            maxval = max_scalar(maxval, v);
            i += 1;
        }
        maxval
    }
}

// ---------------------------------------------------------------------------
// RMSNorm: out[i] = a[i] * w[i] / sqrt(mean(a^2) + eps)
// ---------------------------------------------------------------------------

///
/// # Safety
///
/// Each pointer must be non-null, aligned for `f32`, and valid for the reads
/// and writes that the element counts above imply.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rms_norm_f32(
    a: *const f32,
    w: *const f32,
    out: *mut f32,
    n: u32,
    eps: f32,
) {
    // SAFETY: the caller passes pointers valid for the element counts this
    // export documents, as its C ABI contract states.
    unsafe {
        let n = n as usize;
        let n4 = n & !3;

        // Pass 1: Sum of squares
        let mut acc = f32x4_splat(0.0);
        let mut i = 0usize;
        while i < n4 {
            let va = v128_load(a.add(i) as *const v128);
            acc = f32x4_add(acc, f32x4_mul(va, va));
            i += 4;
        }
        let mut sum_sq = hsum_f32x4(acc);
        while i < n {
            let v = *a.add(i);
            sum_sq += v * v;
            i += 1;
        }

        // scale = 1 / sqrt(sum_sq / n + eps)
        let scale = 1.0 / sqrt_scalar(sum_sq / n as f32 + eps);
        let scale_splat = f32x4_splat(scale);

        // Pass 2: out[i] = a[i] * w[i] * scale
        i = 0;
        while i < n4 {
            let va = v128_load(a.add(i) as *const v128);
            let vw = v128_load(w.add(i) as *const v128);
            v128_store(
                out.add(i) as *mut v128,
                f32x4_mul(f32x4_mul(va, vw), scale_splat),
            );
            i += 4;
        }
        while i < n {
            *out.add(i) = *a.add(i) * *w.add(i) * scale;
            i += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// RoPE rotation
// ---------------------------------------------------------------------------

///
/// # Safety
///
/// Each pointer must be non-null, aligned for `f32`, and valid for the reads
/// and writes that the element counts above imply.
#[unsafe(no_mangle)]
pub unsafe extern "C" fn rope_f32(
    q: *const f32,
    freqs_cos: *const f32,
    freqs_sin: *const f32,
    out: *mut f32,
    n: u32,
) {
    // SAFETY: the caller passes pointers valid for the element counts this
    // export documents, as its C ABI contract states.
    unsafe {
        let half_n = (n / 2) as usize;
        for i in 0..half_n {
            let q0 = *q.add(2 * i);
            let q1 = *q.add(2 * i + 1);
            let c = *freqs_cos.add(i);
            let s = *freqs_sin.add(i);
            *out.add(2 * i) = q0 * c - q1 * s;
            *out.add(2 * i + 1) = q0 * s + q1 * c;
        }
    }
}

// Panic handler for no_std
#[panic_handler]
fn panic(_info: &core::panic::PanicInfo) -> ! {
    core::arch::wasm32::unreachable()
}
