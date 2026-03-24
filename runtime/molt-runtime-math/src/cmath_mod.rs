// === FILE: molt-runtime-math/src/cmath_mod.rs ===
//
// cmath module intrinsics. Complex numbers are passed as two separate f64
// NaN-boxed float arguments (real, imag) and results are returned as a
// 2-element tuple of floats (real_bits, imag_bits).

use molt_runtime_core::prelude::*;
use crate::bridge::*;

// ---------------------------------------------------------------------------
// Internal: build a complex result tuple (real, imag)
// ---------------------------------------------------------------------------

fn complex_tuple(_py: &PyToken, re: f64, im: f64) -> u64 {
    let re_bits = MoltObject::from_float(re).bits();
    let im_bits = MoltObject::from_float(im).bits();
    let tuple_ptr = alloc_tuple(_py, &[re_bits, im_bits]);
    dec_ref_bits(_py, re_bits);
    dec_ref_bits(_py, im_bits);
    if tuple_ptr.is_null() {
        return raise_exception::<u64>(_py, "MemoryError", "out of memory");
    }
    MoltObject::from_ptr(tuple_ptr).bits()
}

fn float_arg(_py: &PyToken, bits: u64, name: &str) -> Result<f64, u64> {
    to_f64(obj_from_bits(bits))
        .ok_or_else(|| raise_exception::<u64>(_py, "TypeError", &format!("{name} must be a float")))
}

// ---------------------------------------------------------------------------
// Complex arithmetic helpers (a + bi)
// ---------------------------------------------------------------------------

// z = a + bi
// sqrt(z): principal square root
fn csqrt(re: f64, im: f64) -> (f64, f64) {
    if im == 0.0 {
        if re >= 0.0 {
            return (re.sqrt(), 0.0);
        } else {
            return (0.0, (-re).sqrt());
        }
    }
    let r = libm::hypot(re, im);
    let out_re = ((r + re) / 2.0).sqrt();
    let out_im = ((r - re) / 2.0).sqrt().copysign(im);
    (out_re, out_im)
}

// exp(a + bi) = e^a * (cos(b) + i*sin(b))
fn cexp(re: f64, im: f64) -> (f64, f64) {
    let exp_re = re.exp();
    (exp_re * im.cos(), exp_re * im.sin())
}

// ln(a + bi) = ln|z| + i*arg(z)
fn clog(re: f64, im: f64) -> (f64, f64) {
    let r = libm::hypot(re, im);
    let theta = im.atan2(re);
    (r.ln(), theta)
}

// log10(z) = ln(z) / ln(10)
fn clog10(re: f64, im: f64) -> (f64, f64) {
    let (lr, li) = clog(re, im);
    let ln10 = std::f64::consts::LN_10;
    (lr / ln10, li / ln10)
}

// sin(a + bi) = sin(a)cosh(b) + i*cos(a)sinh(b)
fn csin(re: f64, im: f64) -> (f64, f64) {
    (re.sin() * libm::cosh(im), re.cos() * libm::sinh(im))
}

// cos(a + bi) = cos(a)cosh(b) - i*sin(a)sinh(b)
// C99 Annex G: ccos(+0+0i) = 1-0i  — must preserve signed-zero imaginary.
fn ccos(re: f64, im: f64) -> (f64, f64) {
    let real = re.cos() * libm::cosh(im);
    let sin_re = re.sin();
    let sinh_im = libm::sinh(im);
    let imag = if sin_re == 0.0 && sinh_im == 0.0 {
        let sign = if sin_re.is_sign_negative() ^ sinh_im.is_sign_negative() {
            1.0_f64
        } else {
            -1.0_f64
        };
        f64::copysign(0.0, sign)
    } else {
        -sin_re * sinh_im
    };
    (real, imag)
}

// tan(z) = sin(z)/cos(z)
fn ctan(re: f64, im: f64) -> (f64, f64) {
    let denom = (2.0 * re).cos() + libm::cosh(2.0 * im);
    if denom == 0.0 {
        return (f64::NAN, f64::NAN);
    }
    ((2.0 * re).sin() / denom, libm::sinh(2.0 * im) / denom)
}

// Complex multiplication
fn cmul(a_re: f64, a_im: f64, b_re: f64, b_im: f64) -> (f64, f64) {
    (a_re * b_re - a_im * b_im, a_re * b_im + a_im * b_re)
}

// Complex division a/b
fn cdiv(a_re: f64, a_im: f64, b_re: f64, b_im: f64) -> (f64, f64) {
    let denom = b_re * b_re + b_im * b_im;
    if denom == 0.0 {
        return (f64::NAN, f64::NAN);
    }
    (
        (a_re * b_re + a_im * b_im) / denom,
        (a_im * b_re - a_re * b_im) / denom,
    )
}

// asin(z) = -i * ln(iz + sqrt(1 - z^2))
fn casin(re: f64, im: f64) -> (f64, f64) {
    let (sq_re, sq_im) = {
        let z2_re = re * re - im * im;
        let z2_im = 2.0 * re * im;
        csqrt(1.0 - z2_re, -z2_im)
    };
    let sum_re = -im + sq_re;
    let sum_im = re + sq_im;
    let (ln_re, ln_im) = clog(sum_re, sum_im);
    (ln_im, -ln_re)
}

// acos(z) = -i * ln(z + i*sqrt(1 - z^2))
fn cacos(re: f64, im: f64) -> (f64, f64) {
    let z2_re = re * re - im * im;
    let z2_im = 2.0 * re * im;
    let (sq_re, sq_im) = csqrt(1.0 - z2_re, -z2_im);
    let sum_re = re + (-sq_im);
    let sum_im = im + sq_re;
    let (ln_re, ln_im) = clog(sum_re, sum_im);
    (ln_im, -ln_re)
}

// atan(z) = i/2 * ln((1-iz)/(1+iz))
fn catan(re: f64, im: f64) -> (f64, f64) {
    let num_re = 1.0 + im;
    let num_im = -re;
    let den_re = 1.0 - im;
    let den_im = re;
    let (q_re, q_im) = cdiv(num_re, num_im, den_re, den_im);
    let (ln_re, ln_im) = clog(q_re, q_im);
    (-ln_im / 2.0, ln_re / 2.0)
}

// sinh(z)
fn csinh(re: f64, im: f64) -> (f64, f64) {
    (libm::sinh(re) * im.cos(), libm::cosh(re) * im.sin())
}

// cosh(z)
fn ccosh(re: f64, im: f64) -> (f64, f64) {
    (libm::cosh(re) * im.cos(), libm::sinh(re) * im.sin())
}

// tanh(z)
fn ctanh(re: f64, im: f64) -> (f64, f64) {
    let denom = (2.0 * re).cosh() + (2.0 * im).cos();
    if denom == 0.0 {
        return (f64::NAN, f64::NAN);
    }
    ((2.0 * re).sinh() / denom, (2.0 * im).sin() / denom)
}

// asinh(z) = ln(z + sqrt(z^2 + 1))
fn casinh(re: f64, im: f64) -> (f64, f64) {
    let z2_re = re * re - im * im;
    let z2_im = 2.0 * re * im;
    let (sq_re, sq_im) = csqrt(z2_re + 1.0, z2_im);
    clog(re + sq_re, im + sq_im)
}

// acosh(z) = ln(z + sqrt(z+1)*sqrt(z-1))
fn cacosh(re: f64, im: f64) -> (f64, f64) {
    let (s1_re, s1_im) = csqrt(re + 1.0, im);
    let (s2_re, s2_im) = csqrt(re - 1.0, im);
    let (prod_re, prod_im) = cmul(s1_re, s1_im, s2_re, s2_im);
    clog(re + prod_re, im + prod_im)
}

// atanh(z) = ln((1+z)/(1-z)) / 2
fn catanh(re: f64, im: f64) -> (f64, f64) {
    let (q_re, q_im) = cdiv(1.0 + re, im, 1.0 - re, -im);
    let (ln_re, ln_im) = clog(q_re, q_im);
    (ln_re / 2.0, ln_im / 2.0)
}

// ---------------------------------------------------------------------------
// Public intrinsics
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_cmath_sqrt(real_bits: u64, imag_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let re = match float_arg(_py, real_bits, "real") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let im = match float_arg(_py, imag_bits, "imag") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let (out_re, out_im) = csqrt(re, im);
        complex_tuple(_py, out_re, out_im)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cmath_exp(real_bits: u64, imag_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let re = match float_arg(_py, real_bits, "real") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let im = match float_arg(_py, imag_bits, "imag") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let (out_re, out_im) = cexp(re, im);
        complex_tuple(_py, out_re, out_im)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cmath_log(real_bits: u64, imag_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let re = match float_arg(_py, real_bits, "real") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let im = match float_arg(_py, imag_bits, "imag") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let (out_re, out_im) = clog(re, im);
        complex_tuple(_py, out_re, out_im)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cmath_log10(real_bits: u64, imag_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let re = match float_arg(_py, real_bits, "real") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let im = match float_arg(_py, imag_bits, "imag") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let (out_re, out_im) = clog10(re, im);
        complex_tuple(_py, out_re, out_im)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cmath_sin(real_bits: u64, imag_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let re = match float_arg(_py, real_bits, "real") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let im = match float_arg(_py, imag_bits, "imag") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let (out_re, out_im) = csin(re, im);
        complex_tuple(_py, out_re, out_im)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cmath_cos(real_bits: u64, imag_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let re = match float_arg(_py, real_bits, "real") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let im = match float_arg(_py, imag_bits, "imag") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let (out_re, out_im) = ccos(re, im);
        complex_tuple(_py, out_re, out_im)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cmath_tan(real_bits: u64, imag_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let re = match float_arg(_py, real_bits, "real") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let im = match float_arg(_py, imag_bits, "imag") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let (out_re, out_im) = ctan(re, im);
        complex_tuple(_py, out_re, out_im)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cmath_asin(real_bits: u64, imag_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let re = match float_arg(_py, real_bits, "real") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let im = match float_arg(_py, imag_bits, "imag") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let (out_re, out_im) = casin(re, im);
        complex_tuple(_py, out_re, out_im)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cmath_acos(real_bits: u64, imag_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let re = match float_arg(_py, real_bits, "real") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let im = match float_arg(_py, imag_bits, "imag") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let (out_re, out_im) = cacos(re, im);
        complex_tuple(_py, out_re, out_im)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cmath_atan(real_bits: u64, imag_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let re = match float_arg(_py, real_bits, "real") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let im = match float_arg(_py, imag_bits, "imag") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let (out_re, out_im) = catan(re, im);
        complex_tuple(_py, out_re, out_im)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cmath_sinh(real_bits: u64, imag_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let re = match float_arg(_py, real_bits, "real") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let im = match float_arg(_py, imag_bits, "imag") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let (out_re, out_im) = csinh(re, im);
        complex_tuple(_py, out_re, out_im)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cmath_cosh(real_bits: u64, imag_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let re = match float_arg(_py, real_bits, "real") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let im = match float_arg(_py, imag_bits, "imag") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let (out_re, out_im) = ccosh(re, im);
        complex_tuple(_py, out_re, out_im)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cmath_tanh(real_bits: u64, imag_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let re = match float_arg(_py, real_bits, "real") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let im = match float_arg(_py, imag_bits, "imag") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let (out_re, out_im) = ctanh(re, im);
        complex_tuple(_py, out_re, out_im)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cmath_asinh(real_bits: u64, imag_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let re = match float_arg(_py, real_bits, "real") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let im = match float_arg(_py, imag_bits, "imag") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let (out_re, out_im) = casinh(re, im);
        complex_tuple(_py, out_re, out_im)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cmath_acosh(real_bits: u64, imag_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let re = match float_arg(_py, real_bits, "real") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let im = match float_arg(_py, imag_bits, "imag") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let (out_re, out_im) = cacosh(re, im);
        complex_tuple(_py, out_re, out_im)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cmath_atanh(real_bits: u64, imag_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let re = match float_arg(_py, real_bits, "real") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let im = match float_arg(_py, imag_bits, "imag") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let (out_re, out_im) = catanh(re, im);
        complex_tuple(_py, out_re, out_im)
    })
}

/// phase(z) = atan2(imag, real)
#[unsafe(no_mangle)]
pub extern "C" fn molt_cmath_phase(real_bits: u64, imag_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let re = match float_arg(_py, real_bits, "real") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let im = match float_arg(_py, imag_bits, "imag") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        MoltObject::from_float(im.atan2(re)).bits()
    })
}

/// polar(z) -> (r, phi) where r = |z|, phi = phase(z)
#[unsafe(no_mangle)]
pub extern "C" fn molt_cmath_polar(real_bits: u64, imag_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let re = match float_arg(_py, real_bits, "real") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let im = match float_arg(_py, imag_bits, "imag") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let r = libm::hypot(re, im);
        let phi = im.atan2(re);
        let r_bits = MoltObject::from_float(r).bits();
        let phi_bits = MoltObject::from_float(phi).bits();
        let tuple_ptr = alloc_tuple(_py, &[r_bits, phi_bits]);
        dec_ref_bits(_py, r_bits);
        dec_ref_bits(_py, phi_bits);
        if tuple_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

/// rect(r, phi) -> (r*cos(phi), r*sin(phi))
#[unsafe(no_mangle)]
pub extern "C" fn molt_cmath_rect(r_bits: u64, phi_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let r = match float_arg(_py, r_bits, "r") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let phi = match float_arg(_py, phi_bits, "phi") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        complex_tuple(_py, r * phi.cos(), r * phi.sin())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cmath_isfinite(real_bits: u64, imag_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let re = match float_arg(_py, real_bits, "real") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let im = match float_arg(_py, imag_bits, "imag") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        MoltObject::from_bool(re.is_finite() && im.is_finite()).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cmath_isinf(real_bits: u64, imag_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let re = match float_arg(_py, real_bits, "real") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let im = match float_arg(_py, imag_bits, "imag") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        MoltObject::from_bool(re.is_infinite() || im.is_infinite()).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cmath_isnan(real_bits: u64, imag_bits: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let re = match float_arg(_py, real_bits, "real") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let im = match float_arg(_py, imag_bits, "imag") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        MoltObject::from_bool(re.is_nan() || im.is_nan()).bits()
    })
}

/// isclose(a, b, rel_tol=1e-09, abs_tol=0.0)
#[unsafe(no_mangle)]
pub extern "C" fn molt_cmath_isclose(a_real: u64, a_imag: u64, b_real: u64, b_imag: u64) -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        let are = match float_arg(_py, a_real, "a.real") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let aim = match float_arg(_py, a_imag, "a.imag") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let bre = match float_arg(_py, b_real, "b.real") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let bim = match float_arg(_py, b_imag, "b.imag") {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let rel_tol = 1e-9f64;
        let abs_tol = 0.0f64;
        let diff = libm::hypot(are - bre, aim - bim);
        let mag_a = libm::hypot(are, aim);
        let mag_b = libm::hypot(bre, bim);
        let threshold = (rel_tol * mag_a.max(mag_b)).max(abs_tol);
        MoltObject::from_bool(diff <= threshold).bits()
    })
}

/// constants() -> tuple (pi, e, tau, inf, infj_real, infj_imag, nan, nanj_real, nanj_imag)
#[unsafe(no_mangle)]
pub extern "C" fn molt_cmath_constants() -> u64 {
    molt_runtime_core::with_gil_entry!(_py, {
        use std::f64::consts;
        let pi = MoltObject::from_float(consts::PI).bits();
        let e = MoltObject::from_float(consts::E).bits();
        let tau = MoltObject::from_float(consts::TAU).bits();
        let inf = MoltObject::from_float(f64::INFINITY).bits();
        let infj_re = MoltObject::from_float(0.0).bits();
        let infj_im = MoltObject::from_float(f64::INFINITY).bits();
        let nan = MoltObject::from_float(f64::NAN).bits();
        let nanj_re = MoltObject::from_float(0.0).bits();
        let nanj_im = MoltObject::from_float(f64::NAN).bits();
        let tuple_ptr = alloc_tuple(
            _py,
            &[pi, e, tau, inf, infj_re, infj_im, nan, nanj_re, nanj_im],
        );
        dec_ref_bits(_py, pi);
        dec_ref_bits(_py, e);
        dec_ref_bits(_py, tau);
        dec_ref_bits(_py, inf);
        dec_ref_bits(_py, infj_re);
        dec_ref_bits(_py, infj_im);
        dec_ref_bits(_py, nan);
        dec_ref_bits(_py, nanj_re);
        dec_ref_bits(_py, nanj_im);
        if tuple_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}
