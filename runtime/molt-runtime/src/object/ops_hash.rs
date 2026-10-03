//! Hash functions — extracted from ops.rs.

use crate::randomness::{fill_os_random, os_random_supported};
use crate::*;
use molt_cpython_abi::abi_types::{Py_hash_t, Py_uhash_t};
use molt_obj_model::MoltObject;
use molt_obj_model::hash_policy::{self, TupleHashAccumulator, normalize_hash as fix_hash};
pub(crate) use molt_obj_model::hash_policy::{
    PY_HASH_BITS, PY_HASH_IMAG, PY_HASH_INF, PY_HASH_MODULUS, PY_HASH_NAN, PY_HASH_NONE,
    PY_HASH_WIDTH, hash_int, hash_pointer,
};
use num_bigint::{BigInt, Sign};
use num_integer::Integer;
use num_traits::{Signed, ToPrimitive};
use std::sync::OnceLock;

pub(crate) struct HashSecret {
    k0: u64,
    k1: u64,
}

// Algorithm metadata stays next to its implementation; the pure object policy
// above owns target widths and the numeric/aggregate primitives.
pub(crate) const PY_HASH_ALGORITHM: &str = "siphash13";
pub(crate) const PY_HASH_ALGORITHM_BITS: u32 = u64::BITS;
pub(crate) const PY_HASH_SEED_BITS: u32 = 2 * u64::BITS;
pub(crate) const PY_HASH_CUTOFF: u32 = 0;
const PY_HASHSEED_MAX: u64 = 4_294_967_295;

static HASH_MODULUS_BIG: OnceLock<BigInt> = OnceLock::new();

fn hash_modulus_big() -> &'static BigInt {
    HASH_MODULUS_BIG.get_or_init(|| BigInt::from(PY_HASH_MODULUS))
}

fn hash_secret(_py: &PyToken<'_>) -> &'static HashSecret {
    runtime_state(_py)
        .hash_secret
        .get_or_init(|| init_hash_secret(_py))
}

fn init_hash_secret(_py: &PyToken<'_>) -> HashSecret {
    match std::env::var("PYTHONHASHSEED") {
        Ok(value) => {
            if value == "random" {
                if !crate::operation_allowed(
                    _py,
                    crate::OperationId::RandomEntropy,
                    crate::audit::AuditArgs::None,
                ) {
                    fatal_hash_seed_capability_denied();
                }
                if !os_random_supported() {
                    fatal_hash_seed_unavailable();
                }
                return random_hash_secret();
            }
            let seed: u32 = value.parse().unwrap_or_else(|_| fatal_hash_seed(&value));
            if seed == 0 {
                return HashSecret { k0: 0, k1: 0 };
            }
            let bytes = lcg_hash_seed(seed);
            HashSecret {
                k0: u64::from_ne_bytes(bytes[..8].try_into().unwrap()),
                k1: u64::from_ne_bytes(bytes[8..].try_into().unwrap()),
            }
        }
        Err(_) => {
            if crate::operation_allowed(
                _py,
                crate::OperationId::RandomEntropy,
                crate::audit::AuditArgs::None,
            ) && os_random_supported()
            {
                random_hash_secret()
            } else {
                HashSecret { k0: 0, k1: 0 }
            }
        }
    }
}

pub(crate) fn fatal_hash_seed(value: &str) -> ! {
    eprintln!(
        "Fatal Python error: PYTHONHASHSEED must be \"random\" or an integer in range [0; {PY_HASHSEED_MAX}]"
    );
    eprintln!("PYTHONHASHSEED={value}");
    std::process::exit(1);
}

fn fatal_hash_seed_unavailable() -> ! {
    eprintln!("Fatal Python error: PYTHONHASHSEED=random is unavailable on wasm-freestanding");
    eprintln!("Use PYTHONHASHSEED=0 or an explicit integer seed.");
    std::process::exit(1);
}

fn fatal_hash_seed_capability_denied() -> ! {
    eprintln!("Fatal Python error: PYTHONHASHSEED=random requires the 'random' capability");
    eprintln!("Grant MOLT_CAPABILITIES=random or select MOLT_CAPABILITY_TIER=full.");
    std::process::exit(1);
}

fn random_hash_secret() -> HashSecret {
    let mut bytes = [0u8; 16];
    if let Err(err) = fill_os_random(&mut bytes) {
        eprintln!("Failed to initialize hash seed: {err}");
        std::process::exit(1);
    }
    HashSecret {
        k0: u64::from_ne_bytes(bytes[..8].try_into().unwrap()),
        k1: u64::from_ne_bytes(bytes[8..].try_into().unwrap()),
    }
}

fn lcg_hash_seed(seed: u32) -> [u8; 16] {
    let mut out = [0u8; 16];
    let mut x = seed;
    for byte in out.iter_mut() {
        x = x.wrapping_mul(214013).wrapping_add(2531011);
        *byte = ((x >> 16) & 0xff) as u8;
    }
    out
}

struct SipHasher13 {
    v0: u64,
    v1: u64,
    v2: u64,
    v3: u64,
    tail: u64,
    ntail: usize,
    total_len: u64,
}

impl SipHasher13 {
    fn new(k0: u64, k1: u64) -> Self {
        Self {
            v0: 0x736f6d6570736575 ^ k0,
            v1: 0x646f72616e646f6d ^ k1,
            v2: 0x6c7967656e657261 ^ k0,
            v3: 0x7465646279746573 ^ k1,
            tail: 0,
            ntail: 0,
            total_len: 0,
        }
    }

    fn sip_round(&mut self) {
        self.v0 = self.v0.wrapping_add(self.v1);
        self.v1 = self.v1.rotate_left(13);
        self.v1 ^= self.v0;
        self.v0 = self.v0.rotate_left(32);
        self.v2 = self.v2.wrapping_add(self.v3);
        self.v3 = self.v3.rotate_left(16);
        self.v3 ^= self.v2;
        self.v0 = self.v0.wrapping_add(self.v3);
        self.v3 = self.v3.rotate_left(21);
        self.v3 ^= self.v0;
        self.v2 = self.v2.wrapping_add(self.v1);
        self.v1 = self.v1.rotate_left(17);
        self.v1 ^= self.v2;
        self.v2 = self.v2.rotate_left(32);
    }

    fn process_block(&mut self, block: u64) {
        self.v3 ^= block;
        self.sip_round();
        self.v0 ^= block;
    }

    fn update(&mut self, bytes: &[u8]) {
        self.total_len = self.total_len.wrapping_add(bytes.len() as u64);
        let mut offset = 0usize;

        // If there's a partial tail from a previous update, fill it first.
        if self.ntail > 0 {
            while offset < bytes.len() && self.ntail < 8 {
                self.tail |= (bytes[offset] as u64) << (8 * self.ntail);
                self.ntail += 1;
                offset += 1;
            }
            if self.ntail == 8 {
                self.process_block(self.tail);
                self.tail = 0;
                self.ntail = 0;
            }
        }

        // Bulk path: process 8-byte blocks directly using little-endian reads.
        // This avoids per-byte shift-and-OR for strings >16 bytes (common for
        // dict keys like module-qualified names, file paths, etc.).
        let remaining = &bytes[offset..];
        let chunks = remaining.len() / 8;
        for i in 0..chunks {
            let block = u64::from_le_bytes([
                remaining[i * 8],
                remaining[i * 8 + 1],
                remaining[i * 8 + 2],
                remaining[i * 8 + 3],
                remaining[i * 8 + 4],
                remaining[i * 8 + 5],
                remaining[i * 8 + 6],
                remaining[i * 8 + 7],
            ]);
            self.process_block(block);
        }
        offset += chunks * 8;

        // Tail: accumulate remaining bytes (0-7).
        for &byte in &bytes[offset..] {
            self.tail |= (byte as u64) << (8 * self.ntail);
            self.ntail += 1;
        }
    }

    fn finish(mut self) -> u64 {
        let b = self.tail | ((self.total_len & 0xff) << 56);
        self.process_block(b);
        self.v2 ^= 0xff;
        for _ in 0..3 {
            self.sip_round();
        }
        self.v0 ^ self.v1 ^ self.v2 ^ self.v3
    }
}

fn reduce_mersenne(mut value: u128) -> u64 {
    let mask = PY_HASH_MODULUS as u128;
    value = (value & mask) + (value >> PY_HASH_BITS);
    value = (value & mask) + (value >> PY_HASH_BITS);
    if value >= mask {
        value -= mask;
    }
    if value == mask { 0 } else { value as u64 }
}

fn mul_mod_mersenne(lhs: u64, rhs: u64) -> u64 {
    reduce_mersenne((lhs as u128) * (rhs as u128))
}

fn hash_bytes_with_secret(bytes: &[u8], secret: &HashSecret) -> i64 {
    if bytes.is_empty() {
        return 0;
    }
    let mut hasher = SipHasher13::new(secret.k0, secret.k1);
    hasher.update(bytes);
    fix_hash(hasher.finish() as i64)
}

fn hash_bytes(_py: &PyToken<'_>, bytes: &[u8]) -> i64 {
    hash_bytes_with_secret(bytes, hash_secret(_py))
}

pub(crate) fn hash_string_bytes(_py: &PyToken<'_>, bytes: &[u8]) -> i64 {
    if bytes.is_empty() {
        return 0;
    }
    let secret = hash_secret(_py);
    let Ok(text) = std::str::from_utf8(bytes) else {
        return hash_bytes_with_secret(bytes, secret);
    };
    // SIMD fast path: if all bytes < 0x80, all codepoints are ASCII (max_codepoint ≤ 0x7F).
    // Use SIMD to check this in bulk rather than iterating char-by-char.
    let max_codepoint = simd_max_byte_value(bytes);
    let mut hasher = SipHasher13::new(secret.k0, secret.k1);
    if max_codepoint <= 0x7f {
        // Pure ASCII: each byte is a codepoint, hash as u8 directly
        hasher.update(bytes);
    } else if max_codepoint <= 0xff {
        for ch in text.chars() {
            hasher.update(&[ch as u8]);
        }
    } else if max_codepoint <= 0xffff {
        for ch in text.chars() {
            let bytes = (ch as u16).to_ne_bytes();
            hasher.update(&bytes);
        }
    } else {
        for ch in text.chars() {
            let bytes = (ch as u32).to_ne_bytes();
            hasher.update(&bytes);
        }
    }
    fix_hash(hasher.finish() as i64)
}

/// SIMD-accelerated max byte value scan. Returns the maximum byte value in the slice.
/// Used to quickly determine string encoding width (ASCII, Latin-1, BMP, full Unicode).
#[inline]
fn simd_max_byte_value(bytes: &[u8]) -> u32 {
    #[cfg(target_arch = "x86_64")]
    {
        if bytes.len() >= 32 && std::arch::is_x86_feature_detected!("avx2") {
            return unsafe { simd_max_byte_avx2(bytes) };
        }
        if bytes.len() >= 16 && std::arch::is_x86_feature_detected!("sse2") {
            return unsafe { simd_max_byte_sse2(bytes) };
        }
    }
    #[cfg(target_arch = "aarch64")]
    {
        if bytes.len() >= 16 && std::arch::is_aarch64_feature_detected!("neon") {
            return unsafe { simd_max_byte_neon(bytes) };
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        if bytes.len() >= 16 {
            return unsafe { simd_max_byte_wasm32(bytes) };
        }
    }
    // Scalar fallback — also handles short strings and decodes actual codepoints
    let mut max = 0u32;
    if let Ok(text) = std::str::from_utf8(bytes) {
        for ch in text.chars() {
            max = max.max(ch as u32);
        }
    } else {
        for &b in bytes {
            max = max.max(b as u32);
        }
    }
    max
}

#[cfg(target_arch = "x86_64")]
unsafe fn simd_max_byte_sse2(bytes: &[u8]) -> u32 {
    unsafe {
        use std::arch::x86_64::*;
        let mut i = 0usize;
        let mut vmax = _mm_setzero_si128();
        while i + 16 <= bytes.len() {
            let v = _mm_loadu_si128(bytes.as_ptr().add(i) as *const __m128i);
            vmax = _mm_max_epu8(vmax, v);
            i += 16;
        }
        // Horizontal max: fold 128 bits down to a single max byte
        let hi64 = _mm_srli_si128(vmax, 8);
        vmax = _mm_max_epu8(vmax, hi64);
        let hi32 = _mm_srli_si128(vmax, 4);
        vmax = _mm_max_epu8(vmax, hi32);
        let hi16 = _mm_srli_si128(vmax, 2);
        vmax = _mm_max_epu8(vmax, hi16);
        let hi8 = _mm_srli_si128(vmax, 1);
        vmax = _mm_max_epu8(vmax, hi8);
        let mut max = (_mm_extract_epi8(vmax, 0) & 0xFF) as u32;
        // Tail bytes
        for &b in &bytes[i..] {
            max = max.max(b as u32);
        }
        // If all bytes < 0x80, return the byte max directly (it's ASCII, so codepoint == byte)
        // If any byte >= 0x80, fall back to full codepoint scan since UTF-8 multi-byte chars
        // could have codepoints > 0xFF
        if max >= 0x80 {
            let mut cp_max = 0u32;
            if let Ok(text) = std::str::from_utf8(bytes) {
                for ch in text.chars() {
                    cp_max = cp_max.max(ch as u32);
                }
            }
            return cp_max;
        }
        max
    }
}

#[cfg(target_arch = "x86_64")]
unsafe fn simd_max_byte_avx2(bytes: &[u8]) -> u32 {
    unsafe {
        use std::arch::x86_64::*;
        let mut i = 0usize;
        let mut vmax = _mm256_setzero_si256();
        while i + 32 <= bytes.len() {
            let v = _mm256_loadu_si256(bytes.as_ptr().add(i) as *const __m256i);
            vmax = _mm256_max_epu8(vmax, v);
            i += 32;
        }
        // Fold 256 to 128
        let lo = _mm256_castsi256_si128(vmax);
        let hi = _mm256_extracti128_si256(vmax, 1);
        let mut v128 = _mm_max_epu8(lo, hi);
        // Fold 128 to single byte
        let hi64 = _mm_srli_si128(v128, 8);
        v128 = _mm_max_epu8(v128, hi64);
        let hi32 = _mm_srli_si128(v128, 4);
        v128 = _mm_max_epu8(v128, hi32);
        let hi16 = _mm_srli_si128(v128, 2);
        v128 = _mm_max_epu8(v128, hi16);
        let hi8 = _mm_srli_si128(v128, 1);
        v128 = _mm_max_epu8(v128, hi8);
        let mut max = (_mm_extract_epi8(v128, 0) & 0xFF) as u32;
        for &b in &bytes[i..] {
            max = max.max(b as u32);
        }
        if max >= 0x80 {
            let mut cp_max = 0u32;
            if let Ok(text) = std::str::from_utf8(bytes) {
                for ch in text.chars() {
                    cp_max = cp_max.max(ch as u32);
                }
            }
            return cp_max;
        }
        max
    }
}

#[cfg(target_arch = "aarch64")]
unsafe fn simd_max_byte_neon(bytes: &[u8]) -> u32 {
    unsafe {
        use std::arch::aarch64::*;
        let mut i = 0usize;
        let mut vmax = vdupq_n_u8(0);
        while i + 16 <= bytes.len() {
            let v = vld1q_u8(bytes.as_ptr().add(i));
            vmax = vmaxq_u8(vmax, v);
            i += 16;
        }
        let mut max = vmaxvq_u8(vmax) as u32;
        for &b in &bytes[i..] {
            max = max.max(b as u32);
        }
        if max >= 0x80 {
            let mut cp_max = 0u32;
            if let Ok(text) = std::str::from_utf8(bytes) {
                for ch in text.chars() {
                    cp_max = cp_max.max(ch as u32);
                }
            }
            return cp_max;
        }
        max
    }
}

#[cfg(target_arch = "wasm32")]
unsafe fn simd_max_byte_wasm32(bytes: &[u8]) -> u32 {
    unsafe {
        use std::arch::wasm32::*;
        let mut i = 0usize;
        let mut vmax = u8x16_splat(0);
        while i + 16 <= bytes.len() {
            let v = v128_load(bytes.as_ptr().add(i) as *const v128);
            vmax = u8x16_max(vmax, v);
            i += 16;
        }
        // Horizontal max: fold 128 bits down to single byte
        let hi64 =
            u8x16_shuffle::<8, 9, 10, 11, 12, 13, 14, 15, 0, 0, 0, 0, 0, 0, 0, 0>(vmax, vmax);
        vmax = u8x16_max(vmax, hi64);
        let hi32 = u8x16_shuffle::<4, 5, 6, 7, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0>(vmax, vmax);
        vmax = u8x16_max(vmax, hi32);
        let hi16 = u8x16_shuffle::<2, 3, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0>(vmax, vmax);
        vmax = u8x16_max(vmax, hi16);
        let hi8 = u8x16_shuffle::<1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0>(vmax, vmax);
        vmax = u8x16_max(vmax, hi8);
        let mut max = u8x16_extract_lane::<0>(vmax) as u32;
        for &b in &bytes[i..] {
            max = max.max(b as u32);
        }
        if max >= 0x80 {
            let mut cp_max = 0u32;
            if let Ok(text) = std::str::from_utf8(bytes) {
                for ch in text.chars() {
                    cp_max = cp_max.max(ch as u32);
                }
            }
            return cp_max;
        }
        max
    }
}

// Object consumers may call this only after proving the exact str hash
// protocol. The object state remains the single string-hash cache.
pub(super) fn hash_string(_py: &PyToken<'_>, ptr: *mut u8) -> i64 {
    let cached = super::object_state(ptr);
    if cached != 0 {
        return cached.wrapping_sub(1);
    }
    let len = unsafe { string_len(ptr) };
    let bytes = unsafe { std::slice::from_raw_parts(string_bytes(ptr), len) };
    let hash = hash_string_bytes(_py, bytes);
    super::object_set_state(ptr, hash.wrapping_add(1));
    hash
}

fn hash_bytes_cached(_py: &PyToken<'_>, ptr: *mut u8, bytes: &[u8]) -> i64 {
    let cached = super::object_state(ptr);
    if cached != 0 {
        return cached.wrapping_sub(1);
    }
    let hash = hash_bytes(_py, bytes);
    super::object_set_state(ptr, hash.wrapping_add(1));
    hash
}

fn hash_bigint(ptr: *mut u8) -> i64 {
    let big = unsafe { bigint_ref(ptr) };
    hash_bigint_value(big)
}

/// CPython `long_hash` (Objects/longobject.c): `(|n| mod _PyHASH_MODULUS)` with
/// the sign re-applied and the `-1 -> -2` fixup. Shared by the BigInt heap path
/// and by [`py_numeric_hash`] (which needs `hash(abs(numerator))`).
pub(crate) fn hash_bigint_value(big: &BigInt) -> i64 {
    let sign = big.sign();
    let modulus = hash_modulus_big();
    let hash = big.abs().mod_floor(modulus);
    let mut hash = hash.to_i64().unwrap_or(0);
    if sign == Sign::Minus {
        hash = -hash;
    }
    fix_hash(hash)
}

/// `|n| mod _PyHASH_MODULUS` as a `u64` in `[0, _PyHASH_MODULUS)`. This is the
/// magnitude of CPython's `hash(abs(n))` for a non-negative argument (no sign,
/// no `-1 -> -2` fixup, since a non-negative residue is never `-1`).
#[cfg(any(feature = "stdlib_math", feature = "stdlib_serial", test))]
fn bigint_abs_mod_modulus(n: &BigInt) -> u64 {
    let modulus = hash_modulus_big();
    // mod_floor on a non-negative dividend with a positive modulus yields a
    // residue in [0, modulus), so to_u64 always succeeds.
    n.abs().mod_floor(modulus).to_u64().unwrap_or(0)
}

/// `base^exp mod _PyHASH_MODULUS` via square-and-multiply over the Mersenne
/// modular multiply. `base` must already be reduced (`< _PyHASH_MODULUS`).
#[cfg(any(feature = "stdlib_math", feature = "stdlib_serial", test))]
fn pow_mod_mersenne(mut base: u64, mut exp: u64) -> u64 {
    let mut result = 1u64;
    while exp > 0 {
        if exp & 1 == 1 {
            result = mul_mod_mersenne(result, base);
        }
        exp >>= 1;
        if exp > 0 {
            base = mul_mod_mersenne(base, base);
        }
    }
    result
}

/// Modular inverse of `q mod _PyHASH_MODULUS` via Fermat's little theorem
/// (`_PyHASH_MODULUS` is the target Mersenne prime), i.e. `q^(M-2) mod M`.
/// `q_mod` must be reduced and non-zero (the caller guarantees `denominator`
/// is not divisible by the modulus before calling).
#[cfg(any(feature = "stdlib_math", feature = "stdlib_serial", test))]
fn modinv_mersenne(q_mod: u64) -> u64 {
    pow_mod_mersenne(q_mod, PY_HASH_MODULUS - 2)
}

/// CPython's exact numeric hash for a rational `numerator / denominator`
/// (`Lib/fractions.py::_hash_algorithm`, generalized so `Decimal.__hash__` and
/// the integer/float entry points all agree). `denominator` MUST be positive
/// (Fraction/Decimal both normalize to a positive denominator).
///
/// ```text
/// if denominator % M == 0:  hash_ = _PyHASH_INF        # no modular inverse
/// else:                     hash_ = (|num| mod M) * inv(den mod M) mod M
/// result = hash_ if num >= 0 else -hash_
/// return -2 if result == -1 else result
/// ```
///
/// `M == _PyHASH_MODULUS` (61 or 31 bits). This is the shared authority for
/// the cross-type invariant `hash(1) == hash(1.0) == hash(Fraction(1)) ==
/// hash(Decimal(1))` and `hash(Fraction(3, 2)) == hash(1.5)`.
#[cfg(any(feature = "stdlib_math", feature = "stdlib_serial", test))]
pub(crate) fn py_numeric_hash(numerator: &BigInt, denominator: &BigInt) -> i64 {
    let den_mod = bigint_abs_mod_modulus(denominator);
    let hash_mag: u64 = if den_mod == 0 {
        // Denominator divisible by the (prime) modulus => no modular inverse;
        // CPython's `pow(den, -1, M)` raises ValueError and the hash becomes
        // _PyHASH_INF (sign applied below). Mirrors the `except ValueError`
        // branch exactly.
        PY_HASH_INF as u64
    } else {
        let num_mod = bigint_abs_mod_modulus(numerator);
        mul_mod_mersenne(num_mod, modinv_mersenne(den_mod))
    };
    let result = if numerator.sign() == Sign::Minus {
        -(hash_mag as i64)
    } else {
        hash_mag as i64
    };
    fix_hash(result)
}

/// CPython's finite `Decimal.__hash__` for `coefficient * 10**exp10`.
///
/// This is intentionally separate from [`py_numeric_hash`]: Decimal exponents
/// can be very large, and materializing `10**abs(exp10)` as a BigInt turns a
/// modular hash into an allocation bomb. CPython hashes Decimals by modular
/// exponentiation, so this path reduces the signed coefficient once and scales
/// by `10**exp10 mod _PyHASH_MODULUS`.
#[cfg(any(feature = "stdlib_serial", test))]
pub(crate) fn py_decimal_hash(coefficient: &BigInt, exp10: i64) -> i64 {
    let coeff_mod = bigint_abs_mod_modulus(coefficient);
    let scale_mod = pow_mod_mersenne(10, exp10.unsigned_abs());
    let hash_mag = if exp10 >= 0 {
        mul_mod_mersenne(coeff_mod, scale_mod)
    } else {
        mul_mod_mersenne(coeff_mod, modinv_mersenne(scale_mod))
    };
    let result = if coefficient.sign() == Sign::Minus {
        -(hash_mag as i64)
    } else {
        hash_mag as i64
    };
    fix_hash(result)
}

fn hash_float(val: f64) -> i64 {
    hash_policy::hash_float(val, PY_HASH_NAN)
}

fn hash_complex(re: f64, im: f64) -> i64 {
    hash_policy::combine_complex_hashes(hash_float(re), hash_float(im))
}

fn hash_tuple(_py: &PyToken<'_>, ptr: *mut u8) -> i64 {
    unsafe {
        crate::object::seq_access::with_immutable_tuple_slice(ptr, |elems| {
            let mut acc = TupleHashAccumulator::new();
            for &elem in elems.iter() {
                let lane = hash_bits_signed(_py, elem);
                if exception_pending(_py) {
                    return 0;
                }
                acc.push(lane);
            }
            acc.finish_tuple(elems.len())
        })
        .unwrap_or(0)
    }
}

fn hash_dataclass_fields(
    _py: &PyToken<'_>,
    fields: &[u64],
    flags: &[u8],
    field_names: &[String],
    type_label: &str,
) -> i64 {
    let mut acc = TupleHashAccumulator::new();
    let mut count = 0usize;
    for (idx, &elem) in fields.iter().enumerate() {
        let flag = flags.get(idx).copied().unwrap_or(0x7);
        if (flag & 0x4) == 0 {
            continue;
        }
        if is_missing_bits(_py, elem) {
            let name = field_names.get(idx).map(|s| s.as_str()).unwrap_or("field");
            let _ = attr_error(_py, type_label, name);
            return 0;
        }
        count += 1;
        let lane = hash_bits_signed(_py, elem);
        if exception_pending(_py) {
            return 0;
        }
        acc.push(lane);
    }
    acc.finish_tuple(count)
}

fn hash_generic_alias(_py: &PyToken<'_>, ptr: *mut u8) -> i64 {
    let origin_bits = unsafe { generic_alias_origin_bits(ptr) };
    let args_bits = unsafe { generic_alias_args_bits(ptr) };
    let mut acc = TupleHashAccumulator::new();
    for lane_bits in [origin_bits, args_bits] {
        let lane = hash_bits_signed(_py, lane_bits);
        if exception_pending(_py) {
            return 0;
        }
        acc.push(lane);
    }
    acc.finish_tuple(2)
}

fn hash_union_type(_py: &PyToken<'_>, ptr: *mut u8) -> i64 {
    let args_bits = unsafe { union_type_args_bits(ptr) };
    let lane = hash_bits_signed(_py, args_bits);
    if exception_pending(_py) {
        return 0;
    }
    let mut acc = TupleHashAccumulator::new();
    acc.push(lane);
    acc.finish_tuple(1)
}

pub(crate) fn hash_slice_bits(
    _py: &PyToken<'_>,
    start_bits: u64,
    stop_bits: u64,
    step_bits: u64,
) -> Option<i64> {
    let mut acc = TupleHashAccumulator::new();
    for bits in [start_bits, stop_bits, step_bits] {
        let lane = hash_bits_signed(_py, bits);
        if exception_pending(_py) {
            return None;
        }
        acc.push(lane);
    }
    Some(acc.finish())
}

fn shuffle_frozenset_hash(hash: Py_uhash_t) -> Py_uhash_t {
    let mixed = (hash ^ 89869747) ^ (hash << 16);
    mixed.wrapping_mul(3644798167)
}

fn hash_frozenset(_py: &PyToken<'_>, ptr: *mut u8) -> i64 {
    let elems = unsafe { set_order(ptr) };
    let mut hash: Py_uhash_t = 0;
    for &elem in elems.iter() {
        let lane = hash_bits_signed(_py, elem);
        if exception_pending(_py) {
            return 0;
        }
        hash ^= shuffle_frozenset_hash(lane as Py_uhash_t);
    }
    // set_order contains only active elements. CPython's null/dummy parity
    // correction belongs to its traversal of empty/deleted table slots.
    hash ^= (elems.len() as Py_uhash_t)
        .wrapping_add(1)
        .wrapping_mul(1927868237);
    hash ^= (hash >> 11) ^ (hash >> 25);
    hash = hash.wrapping_mul(69069).wrapping_add(907133923);
    if hash == Py_uhash_t::MAX {
        hash = 590923713;
    }
    fix_hash(hash as i64)
}

fn hash_unhashable(_py: &PyToken<'_>, obj: MoltObject) -> i64 {
    let name = type_name(_py, obj);
    let msg = format!("unhashable type: '{name}'");
    raise_exception::<_>(_py, "TypeError", &msg)
}

/// Hash a `TYPE_ID_FOREIGN` wrapper by routing to the wrapped C object's own
/// `tp_hash` via the ABI bridge (CPython `PyObject_Hash`). A numpy DType CLASS
/// (a foreign C type whose metatype inherits `type.__hash__`) hashes by identity;
/// a genuinely-unhashable foreign type raises `TypeError` inside the C slot,
/// which we propagate. Without this, foreign objects fell through to
/// `hash_from_dunder`, whose molt-side dict/attr lookup cannot see the C
/// `tp_hash` and wrongly reported `unhashable type` — the numpy.dtypes
/// registration frontier (`PyDict_SetItem(dict, <DType class>, ...)`).
unsafe fn hash_foreign(_py: &PyToken<'_>, obj: MoltObject, ptr: *mut u8) -> i64 {
    let c_ptr = unsafe { crate::object::foreign::foreign_ptr_from_obj(ptr) };
    let h = unsafe { molt_cpython_abi::bridge::molt_foreign_hash(c_ptr) };
    if h == -1 {
        // The C `tp_hash` failed (e.g. a genuinely-unhashable foreign type). The
        // slot left its exception pending; surface a molt exception if the bridge
        // has not already, so the caller propagates rather than masking it.
        if !exception_pending(_py) {
            let name = type_name(_py, obj);
            let msg = format!("unhashable type: '{name}'");
            return raise_exception::<i64>(_py, "TypeError", &msg);
        }
        return 0;
    }
    h as i64
}

fn is_unhashable_type(type_id: u32) -> bool {
    matches!(
        type_id,
        TYPE_ID_LIST
            | crate::TYPE_ID_CELL
            | TYPE_ID_DICT
            | TYPE_ID_SET
            | TYPE_ID_BYTEARRAY
            | TYPE_ID_LIST_BUILDER
            | TYPE_ID_DICT_KEYS_VIEW
            | TYPE_ID_DICT_VALUES_VIEW
            | TYPE_ID_DICT_ITEMS_VIEW
            | TYPE_ID_CALLARGS
    )
}

/// Hash a memoryview, mirroring CPython `Objects/memoryobject.c: memory_hash`.
///
/// A memoryview is hashable iff it is read-only AND its format is a one-byte
/// format (`'B'`, `'b'` or `'c'`); the hash equals the hash of the view's
/// C-contiguous bytes (i.e. `hash(mv.tobytes())`). Error precedence, exception
/// types, and messages are reproduced exactly.
fn hash_memoryview(_py: &PyToken<'_>, ptr: *mut u8) -> i64 {
    unsafe {
        if memoryview_released(ptr) {
            return raise_released_memoryview(_py);
        }
        // CPython runs CHECK_RELEASED_INT first. The released-state check belongs at THIS
        // position, ahead of the writable/format checks, to preserve CPython's
        // error precedence. The `memoryview_collect_bytes` fallback below already
        // raises this same ValueError when the buffer cannot be materialized.

        // Writable views are unhashable. Checked before the format restriction so
        // a writable, non-byte-format view still reports "writable" (CPython order).
        if !memoryview_readonly(ptr) {
            return raise_exception::<i64>(
                _py,
                "ValueError",
                "cannot hash writable memoryview object",
            );
        }

        // Hashing is restricted to the one-byte formats 'B', 'b', 'c'
        // (CPython IS_BYTE_FORMAT). Route through the canonical format parser so
        // the byte-format predicate stays unified with every other memoryview op.
        let is_byte_format = memoryview_format_from_bits(memoryview_format_bits(ptr))
            .is_some_and(|fmt| matches!(fmt.code, b'b' | b'B' | b'c'));
        if !is_byte_format {
            return raise_exception::<i64>(
                _py,
                "ValueError",
                "memoryview: hashing is restricted to formats 'B', 'b' or 'c'",
            );
        }

        // CPython hashes the exporting object and propagates its error: a
        // read-only view over a bytearray (via `.toreadonly()`) raises
        // TypeError: unhashable type: 'bytearray'. The owner's hash value is
        // discarded — only its hashability gates the view. molt flattens nested
        // views at construction, so the owner is always the root bytes/bytearray.
        let _ = hash_bits_signed(_py, memoryview_owner_bits(ptr));
        if exception_pending(_py) {
            return 0;
        }

        // The hash is the hash of the view's C-contiguous bytes (== mv.tobytes()),
        // computed via the shared materialization primitive.
        match memoryview_collect_bytes(ptr) {
            Some(bytes) => hash_bytes(_py, &bytes),
            None => raise_exception::<i64>(
                _py,
                "ValueError",
                "operation forbidden on released memoryview object",
            ),
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum HashDeclaration {
    Builtin,
    Custom,
    Disabled,
}

/// Resolve user declarations before selecting a physical builtin carrier.
/// Implicit __hash__ = None belongs to class construction. Later equality
/// mutation must not disable an inherited hash slot.
unsafe fn hash_declaration(py: &PyToken<'_>, bits: u64) -> HashDeclaration {
    unsafe {
        if let Some(ptr) = obj_from_bits(bits).as_ptr()
            && object_class_bits(ptr) == 0
            && object_type_id(ptr) != TYPE_ID_TYPE
        {
            return HashDeclaration::Builtin;
        }
        let class_bits = type_of_bits(py, bits);
        let Some(class) = obj_from_bits(class_bits).as_ptr() else {
            return HashDeclaration::Builtin;
        };
        if is_builtin_class_bits(py, class_bits) && crate::object::class_is_immutable(py, class) {
            return HashDeclaration::Builtin;
        }
        let hash_name = intern_static_name(py, &runtime_state(py).interned.hash_name, b"__hash__");
        for &base in class_mro_view(py, class).iter() {
            if is_builtin_class_bits(py, base)
                && obj_from_bits(base)
                    .as_ptr()
                    .is_some_and(|base| crate::object::class_is_immutable(py, base))
            {
                break;
            }
            let Some(base) = obj_from_bits(base).as_ptr() else {
                continue;
            };
            let Some(dict) = obj_from_bits(class_dict_bits(base)).as_ptr() else {
                continue;
            };
            if let Some(value) = dict_get_in_place(py, dict, hash_name) {
                return if obj_from_bits(value).is_none() {
                    HashDeclaration::Disabled
                } else {
                    HashDeclaration::Custom
                };
            }
        }
        HashDeclaration::Builtin
    }
}

pub(crate) fn hash_bits_signed(_py: &PyToken<'_>, bits: u64) -> i64 {
    let obj = obj_from_bits(bits);
    if let Some(i) = obj.as_int() {
        return hash_int(i);
    }
    if let Some(b) = obj.as_bool() {
        return hash_int(if b { 1 } else { 0 });
    }
    if obj.is_none() {
        return fix_hash(PY_HASH_NONE);
    }
    if let Some(f) = obj.as_float() {
        return hash_float(f);
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if type_id != TYPE_ID_FOREIGN {
                match hash_declaration(_py, bits) {
                    HashDeclaration::Custom => return hash_from_dunder(_py, obj, ptr).unwrap_or(0),
                    HashDeclaration::Disabled => return hash_unhashable(_py, obj),
                    HashDeclaration::Builtin => {}
                }
                if exception_pending(_py) {
                    return 0;
                }
            }
            if is_unhashable_type(type_id) {
                return hash_unhashable(_py, obj);
            }
            if type_id == TYPE_ID_FOREIGN {
                // A foreign C-extension object hashes by its OWN `tp_hash`
                // (via the ABI bridge → CPython `PyObject_Hash`), never molt's
                // dunder path — molt cannot see the C hash slot.
                return hash_foreign(_py, obj, ptr);
            }
            if type_id == TYPE_ID_STRING {
                return hash_string(_py, ptr);
            }
            if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                return hash_bytes_cached(_py, ptr, bytes);
            }
            if type_id == TYPE_ID_MEMORYVIEW {
                return hash_memoryview(_py, ptr);
            }
            if type_id == TYPE_ID_BIGINT {
                return hash_bigint(ptr);
            }
            if type_id == TYPE_ID_FLOAT {
                let f = crate::object::ops::heap_float_value(ptr);
                return hash_float(f);
            }
            if type_id == TYPE_ID_COMPLEX {
                let value = *complex_ref(ptr);
                return hash_complex(value.re, value.im);
            }
            if type_id == TYPE_ID_TUPLE {
                return hash_tuple(_py, ptr);
            }
            if type_id == TYPE_ID_DATACLASS {
                let desc_ptr = dataclass_desc_ptr(ptr);
                if desc_ptr.is_null() {
                    return hash_pointer(ptr as u64);
                }
                let desc = &*desc_ptr;
                match desc.hash_mode {
                    2 => return hash_unhashable(_py, obj),
                    3 => {
                        return hash_from_dunder(_py, obj, ptr)
                            .unwrap_or_else(|| hash_pointer(ptr as u64));
                    }
                    1 => {
                        let Some(fields) =
                            crate::object::field_storage::dataclass_snapshot(_py, ptr, 0x4)
                        else {
                            return 0;
                        };
                        let type_label = if desc.name.is_empty() {
                            "dataclass"
                        } else {
                            desc.name.as_str()
                        };
                        return hash_dataclass_fields(
                            _py,
                            &fields,
                            &desc.field_flags,
                            &desc.field_names,
                            type_label,
                        );
                    }
                    _ => return hash_pointer(ptr as u64),
                }
            }
            if type_id == TYPE_ID_TYPE {
                return hash_pointer(ptr as u64);
            }
            if type_id == TYPE_ID_GENERIC_ALIAS {
                return hash_generic_alias(_py, ptr);
            }
            if type_id == TYPE_ID_UNION {
                return hash_union_type(_py, ptr);
            }
            if type_id == TYPE_ID_SLICE {
                let start_bits = slice_start_bits(ptr);
                let stop_bits = slice_stop_bits(ptr);
                let step_bits = slice_step_bits(ptr);
                if let Some(hash) = hash_slice_bits(_py, start_bits, stop_bits, step_bits) {
                    return hash;
                }
                return 0;
            }
            if type_id == TYPE_ID_FROZENSET {
                return hash_frozenset(_py, ptr);
            }
            if let Some(hash) = hash_from_dunder(_py, obj, ptr) {
                return hash;
            }
        }
        return hash_pointer(ptr as u64);
    }
    hash_pointer(bits)
}

unsafe fn hash_from_dunder(_py: &PyToken<'_>, obj: MoltObject, obj_ptr: *mut u8) -> Option<i64> {
    unsafe {
        let hash_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.hash_name, b"__hash__");
        let class_bits = type_of_bits(_py, obj.bits());
        let default_type_hashable = class_bits == builtin_classes(_py).type_obj;
        if hash_declaration(_py, obj.bits()) == HashDeclaration::Disabled {
            return Some(hash_unhashable(_py, obj));
        }
        let class_ptr = obj_from_bits(class_bits).as_ptr()?;
        let call_bits =
            class_attr_lookup(_py, class_ptr, class_ptr, Some(obj_ptr), hash_name_bits)?;
        if obj_from_bits(call_bits).is_none() {
            dec_ref_bits(_py, call_bits);
            if default_type_hashable {
                return None;
            }
            let name = type_name(_py, obj);
            let msg = format!("unhashable type: '{name}'");
            return Some(raise_exception::<i64>(_py, "TypeError", &msg));
        }
        let res_bits = call_callable0(_py, call_bits);
        dec_ref_bits(_py, call_bits);
        if exception_pending(_py) {
            if !obj_from_bits(res_bits).is_none() {
                dec_ref_bits(_py, res_bits);
            }
            return Some(0);
        }
        let hash = if let Some(i) = crate::builtins::numbers::index_i64_integral_bits(res_bits) {
            // __hash__ preserves every fitting Py_hash_t, even above the
            // numeric modulus; overflow falls back to integer hashing.
            if Py_hash_t::try_from(i).is_ok() {
                fix_hash(i)
            } else {
                hash_int(i)
            }
        } else if let Some(big) = crate::builtins::numbers::index_bigint_integral_bits(res_bits) {
            hash_bigint_value(&big)
        } else {
            let msg = "__hash__ method should return an integer";
            dec_ref_bits(_py, res_bits);
            return Some(raise_exception::<i64>(_py, "TypeError", msg));
        };
        dec_ref_bits(_py, res_bits);
        Some(hash)
    }
}

pub(crate) fn hash_bits(_py: &PyToken<'_>, bits: u64) -> u64 {
    hash_bits_signed(_py, bits) as u64
}

fn hash_descriptor_type_error(_py: &PyToken<'_>, self_bits: u64, expected: &str) -> u64 {
    let type_label = class_name_for_error(type_of_bits(_py, self_bits));
    let msg = format!(
        "descriptor '__hash__' requires a '{}' object but received '{}'",
        expected, type_label
    );
    raise_exception::<_>(_py, "TypeError", &msg)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_int_hash_method(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(self_bits);
        let value_bits =
            if obj.is_int() || obj.is_bool() || bigint_ptr_from_bits(self_bits).is_some() {
                self_bits
            } else if let Some(bits) = int_subclass_value_bits_raw(self_bits) {
                bits
            } else {
                return hash_descriptor_type_error(_py, self_bits, "int");
            };
        let value = obj_from_bits(value_bits);
        let hash = if let Some(ptr) = bigint_ptr_from_bits(value_bits) {
            hash_bigint(ptr)
        } else {
            hash_int(to_i64(value).expect("validated integer hash receiver"))
        };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        int_bits_from_i64(_py, hash)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_float_hash_method(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(value) = crate::object::ops::as_float_extended(obj_from_bits(self_bits)) else {
            return hash_descriptor_type_error(_py, self_bits, "float");
        };
        let hash = hash_float(value);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        int_bits_from_i64(_py, hash)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_str_hash_method(self_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let obj = obj_from_bits(self_bits);
        let Some(ptr) = obj.as_ptr() else {
            return hash_descriptor_type_error(_py, self_bits, "str");
        };
        unsafe {
            if object_type_id(ptr) != TYPE_ID_STRING {
                return hash_descriptor_type_error(_py, self_bits, "str");
            }
        }
        let hash = hash_string(_py, ptr);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        int_bits_from_i64(_py, hash)
    })
}

/// Operation context for an unhashable-key `TypeError`.
///
/// CPython 3.14 added an operation-specific prefix to the `unhashable type`
/// message: `cannot use 'X' as a set element (unhashable type: 'X')` when the
/// value is inserted into a set, `... as a dict key (...)` for a dict key. The
/// bare form is still emitted on 3.12/3.13 and for operations that merely probe
/// (`set.intersection`/`intersection_update`/`issubset`) without inserting, even
/// on 3.14. `Bare` selects that probe-only form on every version.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum HashContext {
    /// Bare `unhashable type: 'X'` on every version. Used for probe-only set ops
    /// (`intersection`, `intersection_update`, `issubset`) and the `hash()`
    /// builtin.
    Bare,
    /// `cannot use 'X' as a set element (...)` on 3.14; bare on 3.12/3.13.
    SetElement,
    /// `cannot use 'X' as a dict key (...)` on 3.14; bare on 3.12/3.13.
    DictKey,
}

impl HashContext {
    /// The 3.14 context word, or `None` for the bare form (every version).
    #[inline]
    fn word(self) -> Option<&'static str> {
        match self {
            HashContext::Bare => None,
            HashContext::SetElement => Some("set element"),
            HashContext::DictKey => Some("dict key"),
        }
    }
}

/// Render the unhashable-type `TypeError` message for `name`, honoring the
/// operation context and the runtime CPython target version.
#[cold]
#[inline(never)]
pub(crate) fn unhashable_type_message(_py: &PyToken<'_>, name: &str, ctx: HashContext) -> String {
    if let Some(word) = ctx.word()
        && crate::object::ops_sys::runtime_target_at_least(_py, 3, 14)
    {
        format!("cannot use '{name}' as a {word} (unhashable type: '{name}')")
    } else {
        format!("unhashable type: '{name}'")
    }
}

pub(crate) fn ensure_hashable(_py: &PyToken<'_>, key_bits: u64, ctx: HashContext) -> bool {
    let obj = obj_from_bits(key_bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            let declaration = hash_declaration(_py, key_bits);
            if exception_pending(_py) {
                return false;
            }
            if declaration == HashDeclaration::Disabled
                || (declaration == HashDeclaration::Builtin && is_unhashable_type(type_id))
            {
                let name = type_name(_py, obj);
                let msg = unhashable_type_message(_py, &name, ctx);
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    true
}

#[cfg(test)]
mod numeric_hash_tests {
    //! Pins the shared modular numeric hash against CPython 3.12 reference
    //! values on 64-bit targets and checks the ABI-width contract on every
    //! target. The 64-bit golden values are the same constants the differential `fractions_hash_modular.py`,
    //! `decimal_hash_modular.py`, and `numeric_cross_type_hash_invariant.py`
    //! tests assert end-to-end on the compiled binary.
    use super::{
        hash_bigint_value, hash_bytes_cached, hash_float, hash_int, hash_string, pow_mod_mersenne,
        py_decimal_hash, py_numeric_hash,
    };
    use crate::object::{
        ClassEdgeOwnership, ObjectAuxPreselection, TYPE_ID_BYTES, TYPE_ID_STRING,
        alloc_object_with_aux, dec_ref_bits, object_init_class_edge_unpublished, object_state,
    };
    use crate::{MoltObject, builtin_classes};
    use num_bigint::{BigInt, Sign};
    use num_integer::Integer;
    use num_traits::{One, Signed, ToPrimitive};

    fn frac(n: i128, d: i128) -> i64 {
        py_numeric_hash(&BigInt::from(n), &BigInt::from(d))
    }

    // Independent arbitrary-precision arithmetic, rather than the runtime's
    // fixed-word Mersenne multiply/reduce or frexp implementation.
    fn rational_reference(numerator: &BigInt, denominator: &BigInt) -> i64 {
        let modulus = BigInt::from(super::PY_HASH_MODULUS);
        let denominator = denominator.mod_floor(&modulus);
        let magnitude = if denominator == BigInt::from(0) {
            super::PY_HASH_INF
        } else {
            let inverse = denominator.modpow(&(&modulus - 2), &modulus);
            (numerator.abs() * inverse)
                .mod_floor(&modulus)
                .to_i64()
                .unwrap()
        };
        let signed = if numerator.sign() == Sign::Minus {
            -magnitude
        } else {
            magnitude
        };
        if signed == -1 { -2 } else { signed }
    }

    #[test]
    fn numeric_hashes_follow_target_modulus_across_representations() {
        let modulus = super::PY_HASH_MODULUS as i64;
        assert_eq!(
            molt_obj_model::hash_policy::hash_i128(i128::MIN),
            if super::PY_HASH_WIDTH == 64 { -32 } else { -8 },
        );
        assert_eq!(
            molt_obj_model::hash_policy::hash_i128(i128::MAX),
            if super::PY_HASH_WIDTH == 64 { 31 } else { 7 },
        );
        assert_eq!(hash_int(modulus), 0);
        assert_eq!(hash_int(modulus + 1), 1);
        assert_eq!(hash_int(-modulus - 1), -2);
        for value in [i64::MIN, -modulus, -1, 0, 1, 1 << 46, i64::MAX] {
            let integer = BigInt::from(value);
            let expected = rational_reference(&integer, &BigInt::one());
            assert_eq!(hash_int(value), expected);
            assert_eq!(hash_bigint_value(&integer), expected);
            assert_eq!(py_numeric_hash(&integer, &BigInt::one()), expected);
        }
        let one = BigInt::one();
        for (value, numerator, denominator) in [
            (1.5, BigInt::from(3), BigInt::from(2)),
            (-7.0, BigInt::from(-7), one.clone()),
            (
                4503599627370497.0,
                BigInt::from(4503599627370497i64),
                one.clone(),
            ),
            (f64::from_bits(1), one.clone(), &one << 1074usize),
            (
                f64::from_bits(0x000f_ffff_ffff_ffff),
                (&one << 52usize) - 1,
                &one << 1074usize,
            ),
            (f64::MIN_POSITIVE, one.clone(), &one << 1022usize),
            (f64::MAX, ((&one << 53usize) - 1) << 971usize, one.clone()),
        ] {
            let expected = rational_reference(&numerator, &denominator);
            assert_eq!(hash_float(value), expected, "float {value:?}");
            assert_eq!(py_numeric_hash(&numerator, &denominator), expected);
        }
        for (numerator, denominator) in [(1, 3), (-7, 2), (22, 7), (1, modulus)] {
            assert_eq!(
                frac(numerator as i128, denominator as i128),
                rational_reference(&BigInt::from(numerator), &BigInt::from(denominator)),
            );
        }
        for exponent in [-512i64, -31, -1, 0, 1, 31, 512] {
            let coefficient = BigInt::from(-123456789i64);
            let scale = BigInt::from(10).pow(exponent.unsigned_abs() as u32);
            let expected = if exponent >= 0 {
                rational_reference(&(&coefficient * scale), &one)
            } else {
                rational_reference(&coefficient, &scale)
            };
            assert_eq!(py_decimal_hash(&coefficient, exponent), expected);
        }
    }

    #[test]
    fn hash_result_width_and_cpython_siphash_vectors() {
        use super::{HashSecret, PY_HASH_WIDTH, fix_hash, hash_bytes_with_secret, hash_pointer};
        assert_eq!(PY_HASH_WIDTH, molt_cpython_abi::abi_types::Py_hash_t::BITS);
        assert_eq!(fix_hash(-1), -2);
        assert_eq!(hash_pointer(0), 0);
        assert_eq!(
            hash_pointer(molt_cpython_abi::abi_types::Py_uhash_t::MAX as u64),
            -2
        );
        assert_eq!(hash_pointer(1), (1u64 << (PY_HASH_WIDTH - 4)) as i64);
        if PY_HASH_WIDTH == 32 {
            assert_eq!(fix_hash(0x0000_0001_ffff_ffff), -2);
            assert_eq!(fix_hash(0x0000_0000_8000_0000), i32::MIN as i64);
        }
        // CPython v3.12.0 Lib/test/test_hash.py, siphash13 seed=0 'abc'.
        let expected = if PY_HASH_WIDTH == 64 {
            -4594863902769663758
        } else {
            69611762
        };
        assert_eq!(
            hash_bytes_with_secret(b"abc", &HashSecret { k0: 0, k1: 0 }),
            expected,
        );
        assert_eq!(hash_bytes_with_secret(b"", &HashSecret { k0: 0, k1: 0 }), 0);
        assert_eq!(
            super::hash_complex(0.0, 3000.0),
            if PY_HASH_WIDTH == 64 {
                3000009000
            } else {
                -1294958296
            },
        );
        assert_eq!(super::hash_complex(-1000004.0, 1.0), -2);
    }

    #[test]
    fn tuple_hash_consumers_preserve_cpython_target_vectors() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            // CPython v3.12.0 Lib/test/test_tuple.py::test_hash_exact.
            for (values, expected32, expected64) in [
                (vec![], 750394483, 5740354900026072187),
                (
                    vec![MoltObject::from_int(0).bits()],
                    1214856301,
                    -8753497827991233192,
                ),
                (
                    vec![MoltObject::from_int(0).bits(); 2],
                    -168982784,
                    -8458139203682520985,
                ),
                (
                    vec![MoltObject::from_float(0.5).bits()],
                    2077348973,
                    -408149959306781352,
                ),
            ] {
                let tuple = crate::object::builders::alloc_tuple(_py, &values);
                assert!(!tuple.is_null());
                let expected = if super::PY_HASH_WIDTH == 64 {
                    expected64
                } else {
                    expected32
                };
                assert_eq!(super::hash_tuple(_py, tuple), expected);
                assert_eq!(
                    super::hash_dataclass_fields(_py, &values, &[], &[], "fixture"),
                    expected,
                );
                dec_ref_bits(_py, MoltObject::from_ptr(tuple).bits());
            }
        });
    }

    #[test]
    fn frozenset_hashes_only_active_elements_at_target_width() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            // CPython v3.12.0 Objects/setobject.c frozenset_hash, with only
            // active lanes: no empty/dummy table-slot correction remains.
            for (values, expected32, expected64) in [
                (vec![], -1572407560, 133146708735736),
                (vec![0], -281444354, -2704248722033767810),
                (vec![1], 882226578, -558064481276695278),
                (vec![1, 2], -489709338, -1826646154956904602),
                (vec![1, 2, 3], -2021384008, -272375401224217160),
            ] {
                let elems: Vec<u64> = values
                    .into_iter()
                    .map(|value| MoltObject::from_int(value).bits())
                    .collect();
                let set = crate::object::builders::alloc_set_like_with_entries(
                    _py,
                    &elems,
                    crate::TYPE_ID_FROZENSET,
                );
                assert!(!set.is_null());
                let expected = if super::PY_HASH_WIDTH == 64 {
                    expected64
                } else {
                    expected32
                };
                assert_eq!(super::hash_frozenset(_py, set), expected);
                dec_ref_bits(_py, MoltObject::from_ptr(set).bits());
            }
        });
    }

    #[test]
    fn string_and_bytes_subclasses_cache_hash_in_sidecar_state_lane() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let builtins = builtin_classes(_py);
            for (type_id, class_bits, bytes) in [
                (TYPE_ID_STRING, builtins.str, b"subclass-string".as_slice()),
                (TYPE_ID_BYTES, builtins.bytes, b"subclass-bytes".as_slice()),
            ] {
                let total =
                    super::super::layout::InlineBytesStorage::object_size(bytes.len()).unwrap();
                let ptr =
                    alloc_object_with_aux(_py, total, type_id, ObjectAuxPreselection::Sidecar);
                assert!(!ptr.is_null());
                unsafe {
                    super::super::layout::InlineBytesStorage::set_len(ptr, bytes.len());
                    std::ptr::copy_nonoverlapping(
                        bytes.as_ptr(),
                        super::super::layout::InlineBytesStorage::data(ptr),
                        bytes.len(),
                    );
                    assert!(object_init_class_edge_unpublished(
                        _py,
                        ptr,
                        class_bits,
                        ClassEdgeOwnership::Borrowed,
                    ));
                }

                let first = if type_id == TYPE_ID_STRING {
                    hash_string(_py, ptr)
                } else {
                    hash_bytes_cached(_py, ptr, bytes)
                };
                let cached = object_state(ptr);
                assert_ne!(cached, 0, "first hash must populate sidecar state");
                let second = if type_id == TYPE_ID_STRING {
                    hash_string(_py, ptr)
                } else {
                    hash_bytes_cached(_py, ptr, bytes)
                };
                assert_eq!(second, first);
                assert_eq!(object_state(ptr), cached);
                dec_ref_bits(_py, MoltObject::from_ptr(ptr).bits());
            }
        });
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn fraction_hash_matches_cpython() {
        // Whole numbers beyond i64 must NOT collapse to 0.
        let big = BigInt::from(10u8).pow(30);
        assert_eq!(py_numeric_hash(&big, &BigInt::one()), 465258685558744706);
        // A whole Fraction equals its int hash.
        assert_eq!(
            py_numeric_hash(&big, &BigInt::one()),
            hash_bigint_value(&big)
        );

        assert_eq!(frac(1, 3), 1537228672809129301);
        assert_eq!(frac(-7, 2), -1152921504606846979);
        assert_eq!(frac(22, 7), 1976436865040309104);
        assert_eq!(frac(-1, 3), -1537228672809129301);
        assert_eq!(frac(-5, 1), -5);
        assert_eq!(frac(0, 1), 0);

        // Reduction invariant: equal rationals hash identically.
        assert_eq!(frac(2, 6), frac(1, 3));

        // Large denominator beyond i64.
        let d = BigInt::from(10u8).pow(25);
        assert_eq!(py_numeric_hash(&BigInt::one(), &d), 550712693447214898);
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn cross_type_numeric_invariant() {
        // hash(1) == hash(1.0) == hash(Fraction(1)) == hash(Decimal(1))
        assert_eq!(hash_int(1), 1);
        assert_eq!(hash_float(1.0), 1);
        assert_eq!(frac(1, 1), 1);

        // hash(Fraction(3, 2)) == hash(1.5)
        assert_eq!(frac(3, 2), hash_float(1.5));
        assert_eq!(hash_float(1.5), 1152921504606846977);

        // 1/10 has no exact float; Fraction(1,10) and Decimal('0.1') agree via
        // the shared modular authority (computed identically here).
        assert_eq!(frac(1, 10), 2075258708292324556);

        // Negative cross-type.
        assert_eq!(hash_int(-7), -7);
        assert_eq!(hash_float(-7.0), -7);
        assert_eq!(frac(-7, 1), -7);
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn decimal_style_hash_matches_cpython() {
        // Decimal value = coeff * 10^exp, expressed as a rational.
        let ten = BigInt::from(10u8);
        // Decimal('1.5') == 15 / 10
        assert_eq!(
            py_numeric_hash(&BigInt::from(15), &ten),
            1152921504606846977
        );
        // Decimal('-2.5') == -25 / 10
        assert_eq!(
            py_numeric_hash(&BigInt::from(-25), &ten),
            -1152921504606846978
        );
        // Decimal('1E+5') == 100000 / 1
        assert_eq!(
            py_numeric_hash(&BigInt::from(100000), &BigInt::one()),
            100000
        );
        // Decimal('0.1') == 1 / 10 == hash(Fraction(1,10)).
        assert_eq!(py_numeric_hash(&BigInt::one(), &ten), 2075258708292324556);

        assert_eq!(py_decimal_hash(&BigInt::from(15), -1), 1152921504606846977);
        assert_eq!(
            py_decimal_hash(&BigInt::from(-25), -1),
            -1152921504606846978
        );
        assert_eq!(py_decimal_hash(&BigInt::from(1), 5), 100000);
        assert_eq!(py_decimal_hash(&BigInt::one(), -1), 2075258708292324556);
    }

    #[test]
    #[cfg(target_pointer_width = "64")]
    fn decimal_hash_large_exponents_stay_modular() {
        assert_eq!(
            py_decimal_hash(&BigInt::one(), 999_999),
            2137339169833320222
        );
        assert_eq!(
            py_decimal_hash(&BigInt::one(), -999_999),
            2239689609886435038
        );
        assert_eq!(
            py_decimal_hash(&BigInt::from(-1), 999_999),
            -2137339169833320222
        );
        assert_eq!(
            py_decimal_hash(&BigInt::from(-1), -999_999),
            -2239689609886435038
        );
    }

    #[test]
    fn fix_hash_minus_one_maps_to_minus_two() {
        // Find a rational whose modular hash magnitude is exactly 1 with a
        // negative sign so the raw result is -1 and must map to -2. hash of
        // Fraction(-1, 1) is -1 -> -2.
        assert_eq!(frac(-1, 1), -2);
    }

    #[test]
    fn fermat_modular_inverse_roundtrips() {
        // pow_mod_mersenne(q, M-2) is the inverse of q mod M; q * inv % M == 1.
        const M: u64 = super::PY_HASH_MODULUS;
        for q in [2u64, 3, 7, 10, 9999, 1234567891] {
            let inv = pow_mod_mersenne(q, M - 2);
            let prod = ((q as u128) * (inv as u128)) % (M as u128);
            assert_eq!(prod, 1, "modular inverse failed for q={q}");
        }
    }
}
