// === FILE: runtime/molt-runtime/src/builtins/complex_core.rs ===
//
// Complex number type intrinsics. Provides a handle-based state machine for
// complex values (real + imag f64 pair). Integrates with the existing cmath_mod.rs
// arithmetic helpers for transcendental functions.
//
// Handle model: global LazyLock<Mutex<HashMap<i64, ComplexValue>>> keyed by an
// atomically-issued handle ID, returned to Python as a NaN-boxed integer.
//
// WASM compatibility: ALL intrinsics in this module are pure arithmetic with no
// I/O, no file descriptors, no platform-specific syscalls — no `#[cfg]` gating
// required.

use crate::*;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{LazyLock, Mutex};

// ─── Handle counter ─────────────────────────────────────────────────────────

static NEXT_COMPLEX_HANDLE: AtomicI64 = AtomicI64::new(1);

fn next_complex_handle() -> i64 {
    NEXT_COMPLEX_HANDLE.fetch_add(1, Ordering::Relaxed)
}

// ─── Complex value state ────────────────────────────────────────────────────

struct ComplexValue {
    real: f64,
    imag: f64,
}

static COMPLEX_VALUES: LazyLock<Mutex<HashMap<i64, ComplexValue>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

fn with_complex<T>(handle: i64, f: impl FnOnce(&ComplexValue) -> T) -> Option<T> {
    let map = COMPLEX_VALUES.lock().unwrap();
    map.get(&handle).map(f)
}

fn store_complex(real: f64, imag: f64) -> i64 {
    let handle = next_complex_handle();
    let mut map = COMPLEX_VALUES.lock().unwrap();
    map.insert(handle, ComplexValue { real, imag });
    handle
}

// ─── Construction ───────────────────────────────────────────────────────────

/// Create a complex number from real and imag float parts.
/// Returns handle as NaN-boxed int.
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_new(real_bits: u64, imag_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let real = to_f64(obj_from_bits(real_bits)).unwrap_or(0.0);
        let imag = to_f64(obj_from_bits(imag_bits)).unwrap_or(0.0);
        let handle = store_complex(real, imag);
        MoltObject::from_int(handle).bits()
    })
}

/// Parse a complex number from string like "1+2j", "3j", "(1-2j)".
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_from_str(s_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let s = match string_obj_to_owned(obj_from_bits(s_bits)) {
            Some(s) => s,
            None => {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "complex() argument must be a string or a number",
                );
            }
        };
        let trimmed = s.trim();
        // Strip optional outer parens: "(1+2j)" → "1+2j"
        let inner = if trimmed.starts_with('(') && trimmed.ends_with(')') {
            &trimmed[1..trimmed.len() - 1]
        } else {
            trimmed
        };

        match parse_complex(inner) {
            Some((real, imag)) => {
                let handle = store_complex(real, imag);
                MoltObject::from_int(handle).bits()
            }
            None => {
                let msg = format!("complex() arg is a malformed string: {s:?}");
                raise_exception::<u64>(_py, "ValueError", &msg)
            }
        }
    })
}

/// Parse complex string. Handles forms: "1+2j", "1-2j", "2j", "-3j", "1",
/// "1.5+2.5j", "+infj", "nan+nanj", etc.
fn parse_complex(s: &str) -> Option<(f64, f64)> {
    let s = s.trim();
    if s.is_empty() {
        return None;
    }

    // Pure imaginary: "2j", "-3j", "+infj", "nanj", "j"
    if s.ends_with('j') || s.ends_with('J') {
        let imag_str = &s[..s.len() - 1];
        if imag_str.is_empty() || imag_str == "+" {
            return Some((0.0, 1.0));
        }
        if imag_str == "-" {
            return Some((0.0, -1.0));
        }
        // Check if there's a real part too: "1+2j" or "1-2j"
        // Find the last + or - that isn't at position 0 and isn't after 'e'/'E'
        if let Some(split_pos) = find_imag_split(imag_str) {
            let real_str = &imag_str[..split_pos];
            let imag_part = &imag_str[split_pos..];
            let real = parse_float_or_special(real_str)?;
            let imag = if imag_part == "+" {
                1.0
            } else if imag_part == "-" {
                -1.0
            } else {
                parse_float_or_special(imag_part)?
            };
            return Some((real, imag));
        }
        // Pure imaginary
        let imag = parse_float_or_special(imag_str)?;
        return Some((0.0, imag));
    }

    // Pure real
    let real = parse_float_or_special(s)?;
    Some((real, 0.0))
}

/// Find the position of the +/- that separates real from imaginary part.
fn find_imag_split(s: &str) -> Option<usize> {
    let bytes = s.as_bytes();
    let mut i = bytes.len();
    while i > 1 {
        i -= 1;
        if (bytes[i] == b'+' || bytes[i] == b'-')
            && i > 0
            && bytes[i - 1] != b'e'
            && bytes[i - 1] != b'E'
        {
            return Some(i);
        }
    }
    None
}

fn parse_float_or_special(s: &str) -> Option<f64> {
    match s.to_lowercase().as_str() {
        "inf" | "+inf" | "infinity" | "+infinity" => Some(f64::INFINITY),
        "-inf" | "-infinity" => Some(f64::NEG_INFINITY),
        "nan" | "+nan" => Some(f64::NAN),
        "-nan" => Some(-f64::NAN),
        _ => s.parse::<f64>().ok(),
    }
}

// ─── Attribute access ───────────────────────────────────────────────────────

/// Get real part as float.
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_real(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = obj_from_bits(handle_bits).as_int().unwrap_or(0);
        match with_complex(handle, |c| c.real) {
            Some(real) => MoltObject::from_float(real).bits(),
            None => raise_exception::<u64>(_py, "ValueError", "invalid complex handle"),
        }
    })
}

/// Get imaginary part as float.
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_imag(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = obj_from_bits(handle_bits).as_int().unwrap_or(0);
        match with_complex(handle, |c| c.imag) {
            Some(imag) => MoltObject::from_float(imag).bits(),
            None => raise_exception::<u64>(_py, "ValueError", "invalid complex handle"),
        }
    })
}

// ─── Unary operations ───────────────────────────────────────────────────────

/// conjugate() — return (real, -imag)
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_conjugate(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = obj_from_bits(handle_bits).as_int().unwrap_or(0);
        match with_complex(handle, |c| (c.real, c.imag)) {
            Some((real, imag)) => {
                let h = store_complex(real, -imag);
                MoltObject::from_int(h).bits()
            }
            None => raise_exception::<u64>(_py, "ValueError", "invalid complex handle"),
        }
    })
}

/// abs(z) = sqrt(real² + imag²)
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_abs(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = obj_from_bits(handle_bits).as_int().unwrap_or(0);
        match with_complex(handle, |c| libm::hypot(c.real, c.imag)) {
            Some(mag) => MoltObject::from_float(mag).bits(),
            None => raise_exception::<u64>(_py, "ValueError", "invalid complex handle"),
        }
    })
}

/// -z = (-real, -imag)
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_neg(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = obj_from_bits(handle_bits).as_int().unwrap_or(0);
        match with_complex(handle, |c| (c.real, c.imag)) {
            Some((real, imag)) => {
                let h = store_complex(-real, -imag);
                MoltObject::from_int(h).bits()
            }
            None => raise_exception::<u64>(_py, "ValueError", "invalid complex handle"),
        }
    })
}

/// +z = (real, imag) — identity
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_pos(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = obj_from_bits(handle_bits).as_int().unwrap_or(0);
        match with_complex(handle, |c| (c.real, c.imag)) {
            Some((real, imag)) => {
                let h = store_complex(real, imag);
                MoltObject::from_int(h).bits()
            }
            None => raise_exception::<u64>(_py, "ValueError", "invalid complex handle"),
        }
    })
}

/// bool(z) — True if real or imag != 0
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_bool(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = obj_from_bits(handle_bits).as_int().unwrap_or(0);
        match with_complex(handle, |c| c.real != 0.0 || c.imag != 0.0) {
            Some(val) => MoltObject::from_bool(val).bits(),
            None => raise_exception::<u64>(_py, "ValueError", "invalid complex handle"),
        }
    })
}

// ─── Binary arithmetic ──────────────────────────────────────────────────────

/// (a+bi) + (c+di) = (a+c) + (b+d)i
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_add(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ah = obj_from_bits(a_bits).as_int().unwrap_or(0);
        let bh = obj_from_bits(b_bits).as_int().unwrap_or(0);
        let a = match with_complex(ah, |c| (c.real, c.imag)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "ValueError", "invalid complex handle"),
        };
        let b = match with_complex(bh, |c| (c.real, c.imag)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "ValueError", "invalid complex handle"),
        };
        let h = store_complex(a.0 + b.0, a.1 + b.1);
        MoltObject::from_int(h).bits()
    })
}

/// (a+bi) - (c+di) = (a-c) + (b-d)i
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_sub(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ah = obj_from_bits(a_bits).as_int().unwrap_or(0);
        let bh = obj_from_bits(b_bits).as_int().unwrap_or(0);
        let a = match with_complex(ah, |c| (c.real, c.imag)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "ValueError", "invalid complex handle"),
        };
        let b = match with_complex(bh, |c| (c.real, c.imag)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "ValueError", "invalid complex handle"),
        };
        let h = store_complex(a.0 - b.0, a.1 - b.1);
        MoltObject::from_int(h).bits()
    })
}

/// (a+bi)(c+di) = (ac-bd) + (ad+bc)i
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_mul(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ah = obj_from_bits(a_bits).as_int().unwrap_or(0);
        let bh = obj_from_bits(b_bits).as_int().unwrap_or(0);
        let a = match with_complex(ah, |c| (c.real, c.imag)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "ValueError", "invalid complex handle"),
        };
        let b = match with_complex(bh, |c| (c.real, c.imag)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "ValueError", "invalid complex handle"),
        };
        let h = store_complex(a.0 * b.0 - a.1 * b.1, a.0 * b.1 + a.1 * b.0);
        MoltObject::from_int(h).bits()
    })
}

/// (a+bi)/(c+di) using Smith's formula for numerical stability
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_div(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ah = obj_from_bits(a_bits).as_int().unwrap_or(0);
        let bh = obj_from_bits(b_bits).as_int().unwrap_or(0);
        let a = match with_complex(ah, |c| (c.real, c.imag)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "ValueError", "invalid complex handle"),
        };
        let b = match with_complex(bh, |c| (c.real, c.imag)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "ValueError", "invalid complex handle"),
        };
        let denom = b.0 * b.0 + b.1 * b.1;
        if denom == 0.0 {
            return raise_exception::<u64>(_py, "ZeroDivisionError", "complex division by zero");
        }
        let h = store_complex(
            (a.0 * b.0 + a.1 * b.1) / denom,
            (a.1 * b.0 - a.0 * b.1) / denom,
        );
        MoltObject::from_int(h).bits()
    })
}

/// z ** w using e^(w * ln(z))
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_pow(base_bits: u64, exp_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let bh = obj_from_bits(base_bits).as_int().unwrap_or(0);
        let eh = obj_from_bits(exp_bits).as_int().unwrap_or(0);
        let base = match with_complex(bh, |c| (c.real, c.imag)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "ValueError", "invalid complex handle"),
        };
        let exp = match with_complex(eh, |c| (c.real, c.imag)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "ValueError", "invalid complex handle"),
        };

        // Special case: 0 ** 0 = 1+0j
        if exp.0 == 0.0 && exp.1 == 0.0 {
            let h = store_complex(1.0, 0.0);
            return MoltObject::from_int(h).bits();
        }
        // Special case: 0 ** positive_real = 0+0j
        if base.0 == 0.0 && base.1 == 0.0 {
            if exp.0 > 0.0 && exp.1 == 0.0 {
                let h = store_complex(0.0, 0.0);
                return MoltObject::from_int(h).bits();
            }
            return raise_exception::<u64>(
                _py,
                "ZeroDivisionError",
                "0.0 to a negative or complex power",
            );
        }

        // General case: z^w = e^(w * ln(z))
        let ln_r = libm::hypot(base.0, base.1).ln();
        let ln_theta = base.1.atan2(base.0);
        // w * ln(z) = (exp.0 + exp.1*i) * (ln_r + ln_theta*i)
        let mul_re = exp.0 * ln_r - exp.1 * ln_theta;
        let mul_im = exp.0 * ln_theta + exp.1 * ln_r;
        // e^(mul_re + mul_im*i)
        let e_r = mul_re.exp();
        let h = store_complex(e_r * mul_im.cos(), e_r * mul_im.sin());
        MoltObject::from_int(h).bits()
    })
}

// ─── Comparison ─────────────────────────────────────────────────────────────

/// Equality: (a+bi) == (c+di)
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_eq(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ah = obj_from_bits(a_bits).as_int().unwrap_or(0);
        let bh = obj_from_bits(b_bits).as_int().unwrap_or(0);
        let a = match with_complex(ah, |c| (c.real, c.imag)) {
            Some(v) => v,
            None => return MoltObject::from_bool(false).bits(),
        };
        let b = match with_complex(bh, |c| (c.real, c.imag)) {
            Some(v) => v,
            None => return MoltObject::from_bool(false).bits(),
        };
        MoltObject::from_bool(a.0 == b.0 && a.1 == b.1).bits()
    })
}

/// Inequality
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_ne(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ah = obj_from_bits(a_bits).as_int().unwrap_or(0);
        let bh = obj_from_bits(b_bits).as_int().unwrap_or(0);
        let a = match with_complex(ah, |c| (c.real, c.imag)) {
            Some(v) => v,
            None => return MoltObject::from_bool(true).bits(),
        };
        let b = match with_complex(bh, |c| (c.real, c.imag)) {
            Some(v) => v,
            None => return MoltObject::from_bool(true).bits(),
        };
        MoltObject::from_bool(a.0 != b.0 || a.1 != b.1).bits()
    })
}

// ─── repr / str / hash ──────────────────────────────────────────────────────

/// repr(z) — match CPython: "(1+2j)", "(1-2j)", "2j", "(-0+0j)", etc.
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_repr(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = obj_from_bits(handle_bits).as_int().unwrap_or(0);
        let (real, imag) = match with_complex(handle, |c| (c.real, c.imag)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "ValueError", "invalid complex handle"),
        };

        let s = format_complex(real, imag);
        let ptr = alloc_string(_py, s.as_bytes());
        if ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

/// Format a complex number matching CPython repr exactly.
fn format_complex(real: f64, imag: f64) -> String {
    let imag_str = format_float_component(imag);
    if real == 0.0 && !real.is_sign_negative() {
        // Pure imaginary with positive zero real → just "Xj"
        format!("{imag_str}j")
    } else {
        let real_str = format_float_component(real);
        // If imag is positive (or +0.0 or nan), insert explicit '+'
        if imag >= 0.0 || imag.is_nan() {
            format!("({real_str}+{imag_str}j)")
        } else {
            format!("({real_str}{imag_str}j)")
        }
    }
}

fn format_float_component(v: f64) -> String {
    if v.is_nan() {
        return "nan".to_string();
    }
    if v.is_infinite() {
        return if v > 0.0 {
            "inf".to_string()
        } else {
            "-inf".to_string()
        };
    }
    // Match CPython: use 'g' format with enough precision
    let s = format!("{v}");
    // Ensure there's a decimal point for non-integer-like values
    if !s.contains('.') && !s.contains('e') && !s.contains('E') {
        format!("{s}.0")
    } else {
        s
    }
}

/// hash(z) — match CPython: hash(real) XOR hash(imag) * _PyHASH_IMAG
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_hash(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = obj_from_bits(handle_bits).as_int().unwrap_or(0);
        let (real, imag) = match with_complex(handle, |c| (c.real, c.imag)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "ValueError", "invalid complex handle"),
        };

        // CPython's _PyHASH_IMAG constant is _PyHASH_MULTIPLIER which is 1000003
        const HASH_IMAG: i64 = 1000003;
        let hash_real = float_hash(real);
        let hash_imag = float_hash(imag);
        let mut combined = hash_real ^ hash_imag.wrapping_mul(HASH_IMAG);
        if combined == -1 {
            combined = -2;
        }
        MoltObject::from_int(combined).bits()
    })
}

/// CPython-compatible float hash.
fn float_hash(v: f64) -> i64 {
    if v.is_nan() {
        return 0;
    }
    if v.is_infinite() {
        return if v > 0.0 { 314159 } else { -314159 };
    }
    // For integer-valued floats, hash as int
    if v == v.trunc() && v.abs() < (i64::MAX as f64) {
        return v as i64;
    }
    // Use bit representation for non-integer floats
    let bits = v.to_bits() as i64;
    if bits == -1 {
        -2
    } else {
        bits
    }
}

// ─── Mixed arithmetic (complex + scalar) ────────────────────────────────────

/// complex + float/int → complex
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_add_scalar(handle_bits: u64, scalar_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = obj_from_bits(handle_bits).as_int().unwrap_or(0);
        let scalar = to_f64(obj_from_bits(scalar_bits)).unwrap_or(0.0);
        match with_complex(handle, |c| (c.real, c.imag)) {
            Some((real, imag)) => {
                let h = store_complex(real + scalar, imag);
                MoltObject::from_int(h).bits()
            }
            None => raise_exception::<u64>(_py, "ValueError", "invalid complex handle"),
        }
    })
}

/// complex * float/int → complex
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_mul_scalar(handle_bits: u64, scalar_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = obj_from_bits(handle_bits).as_int().unwrap_or(0);
        let scalar = to_f64(obj_from_bits(scalar_bits)).unwrap_or(0.0);
        match with_complex(handle, |c| (c.real, c.imag)) {
            Some((real, imag)) => {
                let h = store_complex(real * scalar, imag * scalar);
                MoltObject::from_int(h).bits()
            }
            None => raise_exception::<u64>(_py, "ValueError", "invalid complex handle"),
        }
    })
}

/// float/int + complex → complex
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_radd_scalar(scalar_bits: u64, handle_bits: u64) -> u64 {
    molt_complex_add_scalar(handle_bits, scalar_bits)
}

/// float/int * complex → complex
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_rmul_scalar(scalar_bits: u64, handle_bits: u64) -> u64 {
    molt_complex_mul_scalar(handle_bits, scalar_bits)
}

// ─── Lifecycle ──────────────────────────────────────────────────────────────

/// Drop/release a complex handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_drop(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = obj_from_bits(handle_bits).as_int().unwrap_or(0);
        let mut map = COMPLEX_VALUES.lock().unwrap();
        map.remove(&handle);
        MoltObject::none().bits()
    })
}

// ─── Conversion helpers ─────────────────────────────────────────────────────

/// Convert complex to tuple (real_bits, imag_bits) for cmath interop.
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_to_tuple(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let handle = obj_from_bits(handle_bits).as_int().unwrap_or(0);
        let (real, imag) = match with_complex(handle, |c| (c.real, c.imag)) {
            Some(v) => v,
            None => return raise_exception::<u64>(_py, "ValueError", "invalid complex handle"),
        };
        let real_bits = MoltObject::from_float(real).bits();
        let imag_bits = MoltObject::from_float(imag).bits();
        let tuple_ptr = alloc_tuple(_py, &[real_bits, imag_bits]);
        dec_ref_bits(_py, real_bits);
        dec_ref_bits(_py, imag_bits);
        if tuple_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

/// Create complex from int (real part, imag=0).
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_from_int(int_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let val = obj_from_bits(int_bits).as_int().unwrap_or(0);
        let h = store_complex(val as f64, 0.0);
        MoltObject::from_int(h).bits()
    })
}

/// Create complex from float (real part, imag=0).
#[unsafe(no_mangle)]
pub extern "C" fn molt_complex_from_float(float_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let val = to_f64(obj_from_bits(float_bits)).unwrap_or(0.0);
        let h = store_complex(val, 0.0);
        MoltObject::from_int(h).bits()
    })
}
