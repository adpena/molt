#![allow(dead_code, unused_imports)]
// === FILE: runtime/molt-runtime/src/builtins/fractions.rs ===
//
// Fraction arithmetic intrinsics using BigInt numerator/denominator.
// Always stores in lowest terms with denominator > 0.

use crate::object::ops::string_obj_to_owned;
use crate::{
    MoltObject, PyToken, alloc_string, alloc_tuple, bits_from_ptr, dec_ref_bits,
    int_bits_from_bigint, int_bits_from_i64, obj_from_bits, ptr_from_bits, raise_exception,
    release_ptr, to_bigint, to_f64, to_i64,
};
use num_bigint::BigInt;
use num_integer::Integer;
use num_traits::{One, Signed, ToPrimitive, Zero};
use std::hash::{Hash, Hasher};

// ---------------------------------------------------------------------------
// FractionHandle – always in lowest terms, denominator > 0
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct FractionHandle {
    numer: BigInt,
    denom: BigInt,
}

impl FractionHandle {
    fn new(numer: BigInt, denom: BigInt) -> Result<Self, &'static str> {
        if denom.is_zero() {
            return Err("Fraction(%s, 0)");
        }
        let g = numer.gcd(&denom);
        let mut n = &numer / &g;
        let mut d = &denom / &g;
        if d.is_negative() {
            n = -n;
            d = -d;
        }
        Ok(FractionHandle { numer: n, denom: d })
    }

    fn add(&self, other: &Self) -> FractionHandle {
        // a/b + c/d = (ad + bc)/(bd), then reduce
        let n = &self.numer * &other.denom + &other.numer * &self.denom;
        let d = &self.denom * &other.denom;
        FractionHandle::new(n, d).unwrap()
    }

    fn sub(&self, other: &Self) -> FractionHandle {
        let n = &self.numer * &other.denom - &other.numer * &self.denom;
        let d = &self.denom * &other.denom;
        FractionHandle::new(n, d).unwrap()
    }

    fn mul(&self, other: &Self) -> FractionHandle {
        let n = &self.numer * &other.numer;
        let d = &self.denom * &other.denom;
        FractionHandle::new(n, d).unwrap()
    }

    fn truediv(&self, other: &Self) -> Result<FractionHandle, &'static str> {
        if other.numer.is_zero() {
            return Err("Fraction(%s, 0)");
        }
        let n = &self.numer * &other.denom;
        let d = &self.denom * &other.numer;
        FractionHandle::new(n, d)
    }

    fn floordiv(&self, other: &Self) -> Result<BigInt, &'static str> {
        if other.numer.is_zero() {
            return Err("Fraction(%s, 0)");
        }
        let n = &self.numer * &other.denom;
        let d = &self.denom * &other.numer;
        Ok(n.div_floor(&d))
    }

    fn modulo(&self, other: &Self) -> Result<FractionHandle, &'static str> {
        let floor = self.floordiv(other)?;
        // self - floor * other
        let floor_frac = FractionHandle {
            numer: floor * &other.denom,
            denom: other.denom.clone(),
        };
        let sub = self.sub(&other.mul(&FractionHandle::new(floor_frac.numer, floor_frac.denom)?));
        Ok(sub)
    }

    fn neg(&self) -> FractionHandle {
        FractionHandle {
            numer: -self.numer.clone(),
            denom: self.denom.clone(),
        }
    }

    fn abs(&self) -> FractionHandle {
        FractionHandle {
            numer: self.numer.abs(),
            denom: self.denom.clone(),
        }
    }

    fn to_f64(&self) -> f64 {
        // compute as string for precision then parse
        if self.denom.is_one() {
            self.numer.to_f64().unwrap_or(f64::NAN)
        } else {
            let n = self.numer.to_f64().unwrap_or(f64::NAN);
            let d = self.denom.to_f64().unwrap_or(f64::NAN);
            n / d
        }
    }

    fn hash_val(&self) -> i64 {
        // CPython Fraction.__hash__ mirrors float hash for whole numbers.
        if self.denom.is_one() {
            return self.numer.to_i64().unwrap_or(0);
        }
        let f = self.to_f64();
        if f.is_finite() {
            // Mirror float hash for exact representable values.
            let bits = f.to_bits();
            bits as i64
        } else {
            // Fallback: hash numerator XOR denominator.
            let mut hasher = std::collections::hash_map::DefaultHasher::new();
            self.numer.to_string().hash(&mut hasher);
            self.denom.to_string().hash(&mut hasher);
            hasher.finish() as i64
        }
    }
}

impl std::fmt::Display for FractionHandle {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.denom.is_one() {
            write!(f, "{}", self.numer)
        } else {
            write!(f, "{}/{}", self.numer, self.denom)
        }
    }
}

// ---------------------------------------------------------------------------
// Float → exact Fraction (CPython fractions.Fraction.from_float semantics)
// ---------------------------------------------------------------------------

fn fraction_from_f64(f: f64) -> Result<FractionHandle, &'static str> {
    if f.is_nan() {
        return Err("cannot convert NaN to Fraction");
    }
    if f.is_infinite() {
        return Err("cannot convert Infinity to Fraction");
    }
    // Decompose f into mantissa * 2^exp (IEEE 754 exact).
    let bits = f.to_bits();
    let sign = if bits >> 63 != 0 { -1i64 } else { 1i64 };
    let exp_raw = ((bits >> 52) & 0x7FF) as i64;
    let mantissa_raw = bits & 0x000F_FFFF_FFFF_FFFF;

    let (mantissa, exp): (i64, i64) = if exp_raw == 0 {
        // subnormal
        (mantissa_raw as i64, -1074)
    } else {
        // normal: implicit leading 1
        (mantissa_raw as i64 | (1i64 << 52), exp_raw - 1075)
    };

    let signed_mantissa = BigInt::from(sign) * BigInt::from(mantissa);
    if exp >= 0 {
        let scale = BigInt::from(2u8).pow(exp as u32);
        FractionHandle::new(signed_mantissa * scale, BigInt::one())
    } else {
        let scale = BigInt::from(2u8).pow((-exp) as u32);
        FractionHandle::new(signed_mantissa, scale)
    }
}

// ---------------------------------------------------------------------------
// Parse string representations: "3/4", "0.75", "-1.5", "1", etc.
// ---------------------------------------------------------------------------

fn fraction_from_str(s: &str) -> Result<FractionHandle, &'static str> {
    let s = s.trim();
    // Try "a/b" format first.
    if let Some(slash_pos) = s.find('/') {
        let num_part = s[..slash_pos].trim();
        let den_part = s[slash_pos + 1..].trim();
        let n: BigInt = num_part
            .parse()
            .map_err(|_| "invalid literal for Fraction")?;
        let d: BigInt = den_part
            .parse()
            .map_err(|_| "invalid literal for Fraction")?;
        return FractionHandle::new(n, d);
    }
    // Try decimal notation.
    let (sign, rest) = if let Some(r) = s.strip_prefix('-') {
        (true, r.trim())
    } else if let Some(r) = s.strip_prefix('+') {
        (false, r.trim())
    } else {
        (false, s)
    };

    // Handle scientific notation.
    let (base_str, exp_val): (&str, i64) = if let Some(e_pos) = rest.to_ascii_lowercase().find('e')
    {
        let (base, exp_part) = rest.split_at(e_pos);
        let ev: i64 = exp_part[1..]
            .parse()
            .map_err(|_| "invalid literal for Fraction")?;
        (base, ev)
    } else {
        (rest, 0)
    };

    let (int_part, frac_part): (&str, &str) = if let Some(dot) = base_str.find('.') {
        (&base_str[..dot], &base_str[dot + 1..])
    } else {
        (base_str, "")
    };

    let combined = format!("{int_part}{frac_part}");
    let numer: BigInt = combined
        .parse()
        .map_err(|_| "invalid literal for Fraction")?;
    let frac_digits = frac_part.len() as i64;
    let effective_exp = exp_val - frac_digits;

    let (n, d) = if effective_exp >= 0 {
        let scale = BigInt::from(10u8).pow(effective_exp as u32);
        (numer * scale, BigInt::one())
    } else {
        let scale = BigInt::from(10u8).pow((-effective_exp) as u32);
        (numer, scale)
    };

    let signed_n = if sign { -n } else { n };
    FractionHandle::new(signed_n, d)
}

// ---------------------------------------------------------------------------
// limit_denominator – CPython-compatible Stern-Brocot approximation
// ---------------------------------------------------------------------------

fn limit_denominator(f: &FractionHandle, max_den: &BigInt) -> FractionHandle {
    if max_den <= &BigInt::zero() || &f.denom <= max_den {
        return f.clone();
    }

    let (mut p0, mut q0, mut p1, mut q1) =
        (BigInt::zero(), BigInt::one(), BigInt::one(), BigInt::zero());

    let (mut n, mut d) = (f.numer.clone(), f.denom.clone());

    loop {
        let a = &n / &d;
        let q2 = &q0 + &a * &q1;
        if &q2 > max_den {
            break;
        }
        let p2 = &p0 + &a * &p1;
        p0 = p1.clone();
        q0 = q1.clone();
        p1 = p2;
        q1 = q2;
        let new_n = d.clone();
        let new_d = n - &a * &d;
        n = new_n;
        d = new_d;
        if d.is_zero() {
            break;
        }
    }

    let k = (max_den - &q0) / &q1;
    let bound1 = FractionHandle::new(&p0 + &k * &p1, &q0 + &k * &q1).unwrap_or(f.clone());
    let bound2 = FractionHandle::new(p1.clone(), q1.clone()).unwrap_or(f.clone());

    // Return the one closest to f.
    let diff1 = {
        let n1 = (&f.numer * &bound1.denom - &bound1.numer * &f.denom).abs();
        let d1 = &f.denom * &bound1.denom;
        (n1, d1)
    };
    let diff2 = {
        let n2 = (&f.numer * &bound2.denom - &bound2.numer * &f.denom).abs();
        let d2 = &f.denom * &bound2.denom;
        (n2, d2)
    };

    // Compare diff1/d1 <= diff2/d2  <=>  diff1.0 * d2 <= diff2.0 * d1
    if &diff1.0 * &diff2.1 <= &diff2.0 * &diff1.1 {
        bound1
    } else {
        bound2
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

fn fraction_handle_from_bits(bits: u64) -> Option<&'static mut FractionHandle> {
    let ptr = ptr_from_bits(bits);
    if ptr.is_null() {
        return None;
    }
    // SAFETY: pointer originates from Box::into_raw for a FractionHandle.
    Some(unsafe { &mut *(ptr as *mut FractionHandle) })
}

fn fraction_bits(handle: FractionHandle) -> u64 {
    bits_from_ptr(Box::into_raw(Box::new(handle)) as *mut u8)
}

fn fraction_from_obj_bits(_py: &PyToken<'_>, bits: u64) -> Result<FractionHandle, u64> {
    // Accept a handle pointer directly (NaN-boxed pointer).
    let ptr = ptr_from_bits(bits);
    if !ptr.is_null() {
        let h = unsafe { &*(ptr as *const FractionHandle) };
        return Ok(h.clone());
    }
    Err(raise_exception::<u64>(
        _py,
        "TypeError",
        "expected Fraction handle",
    ))
}

// ---------------------------------------------------------------------------
// Public intrinsics
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_fraction_new(num_bits: u64, den_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(n) = to_bigint(obj_from_bits(num_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "Fraction numerator must be int");
        };
        let Some(d) = to_bigint(obj_from_bits(den_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "Fraction denominator must be int");
        };
        match FractionHandle::new(n, d) {
            Ok(h) => fraction_bits(h),
            Err(msg) => raise_exception::<u64>(_py, "ZeroDivisionError", msg),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fraction_from_float(f_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(f) = to_f64(obj_from_bits(f_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "argument must be float");
        };
        match fraction_from_f64(f) {
            Ok(h) => fraction_bits(h),
            Err(msg) => raise_exception::<u64>(_py, "ValueError", msg),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fraction_from_str(s_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(s) = string_obj_to_owned(obj_from_bits(s_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "argument must be str");
        };
        match fraction_from_str(&s) {
            Ok(h) => fraction_bits(h),
            Err(msg) => raise_exception::<u64>(_py, "ValueError", msg),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fraction_add(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Ok(a) = fraction_from_obj_bits(_py, a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected Fraction handle");
        };
        let Ok(b) = fraction_from_obj_bits(_py, b_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected Fraction handle");
        };
        fraction_bits(a.add(&b))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fraction_sub(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Ok(a) = fraction_from_obj_bits(_py, a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected Fraction handle");
        };
        let Ok(b) = fraction_from_obj_bits(_py, b_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected Fraction handle");
        };
        fraction_bits(a.sub(&b))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fraction_mul(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Ok(a) = fraction_from_obj_bits(_py, a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected Fraction handle");
        };
        let Ok(b) = fraction_from_obj_bits(_py, b_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected Fraction handle");
        };
        fraction_bits(a.mul(&b))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fraction_truediv(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Ok(a) = fraction_from_obj_bits(_py, a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected Fraction handle");
        };
        let Ok(b) = fraction_from_obj_bits(_py, b_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected Fraction handle");
        };
        match a.truediv(&b) {
            Ok(h) => fraction_bits(h),
            Err(msg) => raise_exception::<u64>(_py, "ZeroDivisionError", msg),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fraction_floordiv(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Ok(a) = fraction_from_obj_bits(_py, a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected Fraction handle");
        };
        let Ok(b) = fraction_from_obj_bits(_py, b_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected Fraction handle");
        };
        match a.floordiv(&b) {
            Ok(big) => int_bits_from_bigint(_py, big),
            Err(msg) => raise_exception::<u64>(_py, "ZeroDivisionError", msg),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fraction_mod(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Ok(a) = fraction_from_obj_bits(_py, a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected Fraction handle");
        };
        let Ok(b) = fraction_from_obj_bits(_py, b_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected Fraction handle");
        };
        match a.modulo(&b) {
            Ok(h) => fraction_bits(h),
            Err(msg) => raise_exception::<u64>(_py, "ZeroDivisionError", msg),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fraction_pow(a_bits: u64, exp_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Ok(base) = fraction_from_obj_bits(_py, a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected Fraction handle");
        };
        let Some(exp) = to_i64(obj_from_bits(exp_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "exponent must be int");
        };
        let result = if exp == 0 {
            FractionHandle {
                numer: BigInt::one(),
                denom: BigInt::one(),
            }
        } else if exp > 0 {
            let n = base.numer.pow(exp as u32);
            let d = base.denom.pow(exp as u32);
            FractionHandle::new(n, d).unwrap()
        } else {
            // negative exponent: (num/den)^-k = den^k/num^k
            let k = (-exp) as u32;
            let n = base.denom.pow(k);
            let d = base.numer.pow(k);
            match FractionHandle::new(n, d) {
                Ok(h) => h,
                Err(msg) => return raise_exception::<u64>(_py, "ZeroDivisionError", msg),
            }
        };
        fraction_bits(result)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fraction_neg(a_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Ok(a) = fraction_from_obj_bits(_py, a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected Fraction handle");
        };
        fraction_bits(a.neg())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fraction_abs(a_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Ok(a) = fraction_from_obj_bits(_py, a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected Fraction handle");
        };
        fraction_bits(a.abs())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fraction_eq(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Ok(a) = fraction_from_obj_bits(_py, a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected Fraction handle");
        };
        let Ok(b) = fraction_from_obj_bits(_py, b_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected Fraction handle");
        };
        MoltObject::from_bool(a.numer == b.numer && a.denom == b.denom).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fraction_lt(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Ok(a) = fraction_from_obj_bits(_py, a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected Fraction handle");
        };
        let Ok(b) = fraction_from_obj_bits(_py, b_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected Fraction handle");
        };
        // a/b < c/d  <=>  ad < bc  (denominators always positive)
        let lhs = &a.numer * &b.denom;
        let rhs = &b.numer * &a.denom;
        MoltObject::from_bool(lhs < rhs).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fraction_le(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Ok(a) = fraction_from_obj_bits(_py, a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected Fraction handle");
        };
        let Ok(b) = fraction_from_obj_bits(_py, b_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected Fraction handle");
        };
        let lhs = &a.numer * &b.denom;
        let rhs = &b.numer * &a.denom;
        MoltObject::from_bool(lhs <= rhs).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fraction_numerator(a_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Ok(a) = fraction_from_obj_bits(_py, a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected Fraction handle");
        };
        int_bits_from_bigint(_py, a.numer.clone())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fraction_denominator(a_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Ok(a) = fraction_from_obj_bits(_py, a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected Fraction handle");
        };
        int_bits_from_bigint(_py, a.denom.clone())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fraction_to_float(a_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Ok(a) = fraction_from_obj_bits(_py, a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected Fraction handle");
        };
        MoltObject::from_float(a.to_f64()).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fraction_to_str(a_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Ok(a) = fraction_from_obj_bits(_py, a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected Fraction handle");
        };
        let s = a.to_string();
        let ptr = alloc_string(_py, s.as_bytes());
        if ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fraction_limit_denominator(a_bits: u64, max_den_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Ok(a) = fraction_from_obj_bits(_py, a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected Fraction handle");
        };
        let Some(max_den_big) = to_bigint(obj_from_bits(max_den_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "max_denominator must be int");
        };
        if max_den_big <= BigInt::zero() {
            return raise_exception::<u64>(_py, "ValueError", "max_denominator must be positive");
        }
        let result = limit_denominator(&a, &max_den_big);
        fraction_bits(result)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fraction_as_integer_ratio(a_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Ok(a) = fraction_from_obj_bits(_py, a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected Fraction handle");
        };
        let num_bits = int_bits_from_bigint(_py, a.numer.clone());
        let den_bits = int_bits_from_bigint(_py, a.denom.clone());
        let tuple_ptr = alloc_tuple(_py, &[num_bits, den_bits]);
        dec_ref_bits(_py, num_bits);
        dec_ref_bits(_py, den_bits);
        if tuple_ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fraction_hash(a_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Ok(a) = fraction_from_obj_bits(_py, a_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "expected Fraction handle");
        };
        int_bits_from_i64(_py, a.hash_val())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_fraction_drop(a_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let ptr = ptr_from_bits(a_bits);
        if ptr.is_null() {
            return MoltObject::none().bits();
        }
        release_ptr(ptr);
        // SAFETY: pointer is owned by this runtime.
        unsafe {
            drop(Box::from_raw(ptr as *mut FractionHandle));
        }
        MoltObject::none().bits()
    })
}
