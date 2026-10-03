//! Pure target hash policy shared by runtime objects, leaf crates, and C ABI slots.
//!
//! Py_hash_t determines public results; SipHash may retain wider internal words.
//! Functions return an i64 transport value already normalized to that ABI so
//! runtime object boxing and C callers cannot choose different width policies.

/// Target signed Python hash word; the C ABI aliases Py_hash_t to this type.
pub type HashWord = isize;
/// Unsigned arithmetic word used by Python's aggregate hash algorithms.
pub type UnsignedHashWord = usize;

pub const PY_HASH_WIDTH: u32 = HashWord::BITS;
pub const PY_HASH_BITS: u32 = if PY_HASH_WIDTH >= 64 { 61 } else { 31 };
pub const PY_HASH_MODULUS: u64 = (1u64 << PY_HASH_BITS) - 1;
pub const PY_HASH_INF: i64 = 314_159;
pub const PY_HASH_NAN: i64 = 0;
pub const PY_HASH_IMAG: i64 = 1_000_003;
pub const PY_HASH_NONE: i64 = 0xfca86420;

/// Narrow to the target signed word before replacing the error sentinel.
#[inline]
pub fn normalize_hash(hash: i64) -> i64 {
    let hash = hash as HashWord;
    if hash == -1 { -2 } else { hash as i64 }
}

#[inline]
pub fn hash_pointer(ptr: u64) -> i64 {
    normalize_hash((ptr as UnsignedHashWord).rotate_right(4) as i64)
}

#[inline]
pub fn combine_complex_hashes(real: i64, imag: i64) -> i64 {
    let hash = (real as UnsignedHashWord)
        .wrapping_add((imag as UnsignedHashWord).wrapping_mul(PY_HASH_IMAG as UnsignedHashWord));
    normalize_hash(hash as i64)
}

#[inline]
pub fn hash_int(val: i64) -> i64 {
    // Fast path: for values whose magnitude fits within PY_HASH_MODULUS
    // (including all inline ints on 64-bit targets), skip the i128
    // modulus arithmetic entirely.
    if val >= 0 && (val as u64) < PY_HASH_MODULUS {
        return val; // val >= 0 so val != -1, normalization not needed
    }
    if val < 0 && val != i64::MIN {
        let mag = (-val) as u64;
        if mag < PY_HASH_MODULUS {
            return normalize_hash(val); // handles -1 -> -2
        }
    }
    hash_i128(val as i128)
}

/// Signed integer reduction, including i128::MIN without signed overflow.
#[inline]
pub fn hash_i128(val: i128) -> i64 {
    let magnitude = val.unsigned_abs();
    let residue = if magnitude < PY_HASH_MODULUS as u128 {
        magnitude as i64
    } else {
        (magnitude % PY_HASH_MODULUS as u128) as i64
    };
    normalize_hash(if val < 0 { -residue } else { residue })
}

fn frexp_abs(value: f64) -> (f64, i32) {
    if value == 0.0 {
        return (0.0, 0);
    }
    let bits = value.to_bits();
    let exponent = ((bits >> 52) & 0x7ff) as i32;
    let mantissa = bits & ((1u64 << 52) - 1);
    if exponent == 0 {
        let (m, e) = frexp_abs(value * ((1u64 << 54) as f64));
        return (m, e - 54);
    }
    let m = f64::from_bits((1022u64 << 52) | mantissa);
    (m, exponent - 1022)
}

/// Shared numeric hashing; callers supply their representation's NaN identity.
#[inline]
pub fn hash_float(v: f64, nan_hash: i64) -> i64 {
    if v.is_infinite() {
        return if v.is_sign_positive() {
            PY_HASH_INF
        } else {
            -PY_HASH_INF
        };
    }
    if v.is_nan() {
        return normalize_hash(nan_hash);
    }

    let sign = if v.is_sign_negative() { -1 } else { 1 };
    let (mut mantissa, mut exponent) = frexp_abs(v.abs());
    let hash_bits = PY_HASH_BITS;
    let modulus = PY_HASH_MODULUS;
    let mut hash = 0u64;

    while mantissa != 0.0 {
        hash = ((hash << 28) & modulus) | (hash >> (hash_bits - 28));
        mantissa *= 268_435_456.0;
        exponent -= 28;
        let chunk = mantissa as u64;
        mantissa -= chunk as f64;
        hash += chunk;
        if hash >= modulus {
            hash -= modulus;
        }
    }

    let rotate = if exponent >= 0 {
        (exponent as u32) % hash_bits
    } else {
        hash_bits - 1 - ((-1 - exponent) as u32 % hash_bits)
    };
    if rotate != 0 {
        hash = ((hash << rotate) & modulus) | (hash >> (hash_bits - rotate));
    }

    let signed = if sign < 0 {
        -(hash as i64)
    } else {
        hash as i64
    };
    normalize_hash(signed)
}

// One target-width accumulator owns tuple-family mixing. Callers retain their
// existing lane selection and finish policy (slices omit tuple length mixing).
pub struct TupleHashAccumulator(UnsignedHashWord);

impl Default for TupleHashAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

impl TupleHashAccumulator {
    const PRIME_1: UnsignedHashWord = if PY_HASH_WIDTH == 64 {
        11400714785074694791u64 as UnsignedHashWord
    } else {
        2654435761
    };
    const PRIME_2: UnsignedHashWord = if PY_HASH_WIDTH == 64 {
        14029467366897019727u64 as UnsignedHashWord
    } else {
        2246822519
    };
    const PRIME_5: UnsignedHashWord = if PY_HASH_WIDTH == 64 {
        2870177450012600261u64 as UnsignedHashWord
    } else {
        3747613939
    };
    const ROTATE: u32 = if PY_HASH_WIDTH == 64 { 31 } else { 13 };

    #[inline]
    pub fn new() -> Self {
        Self(Self::PRIME_5)
    }

    #[inline]
    pub fn push(&mut self, lane: i64) {
        self.0 = self
            .0
            .wrapping_add((lane as UnsignedHashWord).wrapping_mul(Self::PRIME_2));
        self.0 = self.0.rotate_left(Self::ROTATE).wrapping_mul(Self::PRIME_1);
    }

    #[inline]
    pub fn finish(self) -> i64 {
        if self.0 == UnsignedHashWord::MAX {
            1546275796
        } else {
            normalize_hash(self.0 as i64)
        }
    }

    #[inline]
    pub fn finish_tuple(mut self, len: usize) -> i64 {
        self.0 = self
            .0
            .wrapping_add((len as UnsignedHashWord) ^ (Self::PRIME_5 ^ 3527539));
        self.finish()
    }
}
