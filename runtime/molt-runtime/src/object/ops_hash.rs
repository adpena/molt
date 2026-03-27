//! Hash functions — extracted from ops.rs.

use crate::randomness::{fill_os_random, os_random_supported};
use crate::*;
use molt_obj_model::MoltObject;
use num_bigint::{BigInt, Sign};
use num_integer::Integer;
use num_traits::{Signed, ToPrimitive};
use std::sync::OnceLock;

pub(crate) struct HashSecret {
    k0: u64,
    k1: u64,
}

const PY_HASH_BITS: u32 = 61;
const PY_HASH_MODULUS: u64 = (1u64 << PY_HASH_BITS) - 1;
const PY_HASH_INF: i64 = 314_159;
const PY_HASH_NONE: i64 = 0xfca86420;
const PY_HASHSEED_MAX: u64 = 4_294_967_295;

static HASH_MODULUS_BIG: OnceLock<BigInt> = OnceLock::new();

fn hash_modulus_big() -> &'static BigInt {
    HASH_MODULUS_BIG.get_or_init(|| BigInt::from(PY_HASH_MODULUS))
}

fn hash_secret(_py: &PyToken<'_>) -> &'static HashSecret {
    runtime_state(_py).hash_secret.get_or_init(init_hash_secret)
}

fn init_hash_secret() -> HashSecret {
    match std::env::var("PYTHONHASHSEED") {
        Ok(value) => {
            if value == "random" {
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
            if os_random_supported() {
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

fn fix_hash(hash: i64) -> i64 {
    if hash == -1 { -2 } else { hash }
}

fn exp_mod(exp: i32) -> u32 {
    if exp >= 0 {
        (exp as u32) % PY_HASH_BITS
    } else {
        PY_HASH_BITS - 1 - ((-1 - exp) as u32 % PY_HASH_BITS)
    }
}

fn pow2_mod(exp: u32) -> u64 {
    let mut value = 1u64;
    for _ in 0..exp {
        value <<= 1;
        if value >= PY_HASH_MODULUS {
            value -= PY_HASH_MODULUS;
        }
    }
    value
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

fn frexp(value: f64) -> (f64, i32) {
    if value == 0.0 {
        return (0.0, 0);
    }
    let bits = value.to_bits();
    let mut exp = ((bits >> 52) & 0x7ff) as i32;
    let mut mant = bits & ((1u64 << 52) - 1);
    if exp == 0 {
        let mut e = -1022;
        while mant & (1u64 << 52) == 0 {
            mant <<= 1;
            e -= 1;
        }
        exp = e;
        mant &= (1u64 << 52) - 1;
    } else {
        exp -= 1022;
    }
    let frac_bits = (1022u64 << 52) | mant;
    let frac = f64::from_bits(frac_bits);
    (frac, exp)
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

#[cfg(target_arch = "x86_64")]
unsafe fn simd_max_byte_avx2(bytes: &[u8]) -> u32 {
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

fn hash_string(_py: &PyToken<'_>, ptr: *mut u8) -> i64 {
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

pub(crate) fn hash_int(val: i64) -> i64 {
    // Fast path: for values whose magnitude fits within PY_HASH_MODULUS
    // (which includes all 47-bit inline NaN-boxed ints), skip the i128
    // modulus arithmetic entirely.
    if val >= 0 && (val as u64) < PY_HASH_MODULUS {
        return val; // val >= 0 so val != -1, fix_hash not needed
    }
    if val < 0 && val != i64::MIN {
        let mag = (-val) as u64;
        if mag < PY_HASH_MODULUS {
            return fix_hash(val); // fix_hash handles -1 -> -2
        }
    }
    let mut mag = val as i128;
    let sign = if mag < 0 { -1 } else { 1 };
    if mag < 0 {
        mag = -mag;
    }
    let modulus = PY_HASH_MODULUS as i128;
    let mut hash = (mag % modulus) as i64;
    if sign < 0 {
        hash = -hash;
    }
    fix_hash(hash)
}

fn hash_bigint(ptr: *mut u8) -> i64 {
    let big = unsafe { bigint_ref(ptr) };
    let sign = big.sign();
    let modulus = hash_modulus_big();
    let hash = big.abs().mod_floor(modulus);
    let mut hash = hash.to_i64().unwrap_or(0);
    if sign == Sign::Minus {
        hash = -hash;
    }
    fix_hash(hash)
}

fn hash_float(val: f64) -> i64 {
    if val.is_nan() {
        return 0;
    }
    if val.is_infinite() {
        return if val.is_sign_positive() {
            PY_HASH_INF
        } else {
            -PY_HASH_INF
        };
    }
    if val == 0.0 {
        return 0;
    }
    let value = val.abs();
    let mut sign = 1i64;
    if val.is_sign_negative() {
        sign = -1;
    }
    let (mut frac, mut exp) = frexp(value);
    let mut hash = 0u64;
    while frac != 0.0 {
        frac *= (1u64 << 28) as f64;
        let intpart = frac as u64;
        frac -= intpart as f64;
        hash = ((hash << 28) & PY_HASH_MODULUS) | intpart;
        exp -= 28;
    }
    let exp = exp_mod(exp);
    hash = mul_mod_mersenne(hash, pow2_mod(exp));
    let hash = (hash as i64) * sign;
    fix_hash(hash)
}

fn hash_complex(re: f64, im: f64) -> i64 {
    let re_hash = hash_float(re);
    let im_hash = hash_float(im);
    let mut hash = re_hash.wrapping_add(im_hash.wrapping_mul(1000003));
    if hash == -1 {
        hash = -2;
    }
    hash
}

fn hash_tuple(_py: &PyToken<'_>, ptr: *mut u8) -> i64 {
    let elems = unsafe { seq_vec_ref(ptr) };
    #[cfg(target_pointer_width = "64")]
    {
        const XXPRIME_1: u64 = 11400714785074694791;
        const XXPRIME_2: u64 = 14029467366897019727;
        const XXPRIME_5: u64 = 2870177450012600261;
        let mut acc = XXPRIME_5;
        for &elem in elems.iter() {
            let lane = hash_bits_signed(_py, elem);
            if exception_pending(_py) {
                return 0;
            }
            acc = acc.wrapping_add((lane as u64).wrapping_mul(XXPRIME_2));
            acc = acc.rotate_left(31);
            acc = acc.wrapping_mul(XXPRIME_1);
        }
        acc = acc.wrapping_add((elems.len() as u64) ^ (XXPRIME_5 ^ 3527539));
        if acc == u64::MAX {
            return 1546275796;
        }
        acc as i64
    }
    #[cfg(target_pointer_width = "32")]
    {
        const XXPRIME_1: u32 = 2654435761;
        const XXPRIME_2: u32 = 2246822519;
        const XXPRIME_5: u32 = 374761393;
        let mut acc = XXPRIME_5;
        for &elem in elems.iter() {
            let lane = hash_bits_signed(_py, elem);
            if exception_pending(_py) {
                return 0;
            }
            acc = acc.wrapping_add((lane as u32).wrapping_mul(XXPRIME_2));
            acc = acc.rotate_left(13);
            acc = acc.wrapping_mul(XXPRIME_1);
        }
        acc = acc.wrapping_add((elems.len() as u32) ^ (XXPRIME_5 ^ 3527539));
        if acc == u32::MAX {
            return 1546275796;
        }
        (acc as i32) as i64
    }
}

fn hash_dataclass_fields(
    _py: &PyToken<'_>,
    fields: &[u64],
    flags: &[u8],
    field_names: &[String],
    type_label: &str,
) -> i64 {
    #[cfg(target_pointer_width = "64")]
    {
        const XXPRIME_1: u64 = 11400714785074694791;
        const XXPRIME_2: u64 = 14029467366897019727;
        const XXPRIME_5: u64 = 2870177450012600261;
        let mut acc = XXPRIME_5;
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
            acc = acc.wrapping_add((lane as u64).wrapping_mul(XXPRIME_2));
            acc = acc.rotate_left(31);
            acc = acc.wrapping_mul(XXPRIME_1);
        }
        acc = acc.wrapping_add((count as u64) ^ (XXPRIME_5 ^ 3527539));
        if acc == u64::MAX {
            return 1546275796;
        }
        acc as i64
    }
    #[cfg(target_pointer_width = "32")]
    {
        const XXPRIME_1: u32 = 2654435761;
        const XXPRIME_2: u32 = 2246822519;
        const XXPRIME_5: u32 = 374761393;
        let mut acc = XXPRIME_5;
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
            acc = acc.wrapping_add((lane as u32).wrapping_mul(XXPRIME_2));
            acc = acc.rotate_left(13);
            acc = acc.wrapping_mul(XXPRIME_1);
        }
        acc = acc.wrapping_add((count as u32) ^ (XXPRIME_5 ^ 3527539));
        if acc == u32::MAX {
            return 1546275796;
        }
        (acc as i32) as i64
    }
}

fn hash_generic_alias(_py: &PyToken<'_>, ptr: *mut u8) -> i64 {
    let origin_bits = unsafe { generic_alias_origin_bits(ptr) };
    let args_bits = unsafe { generic_alias_args_bits(ptr) };
    #[cfg(target_pointer_width = "64")]
    {
        const XXPRIME_1: u64 = 11400714785074694791;
        const XXPRIME_2: u64 = 14029467366897019727;
        const XXPRIME_5: u64 = 2870177450012600261;
        let mut acc = XXPRIME_5;
        for lane_bits in [origin_bits, args_bits] {
            let lane = hash_bits_signed(_py, lane_bits);
            if exception_pending(_py) {
                return 0;
            }
            acc = acc.wrapping_add((lane as u64).wrapping_mul(XXPRIME_2));
            acc = acc.rotate_left(31);
            acc = acc.wrapping_mul(XXPRIME_1);
        }
        acc = acc.wrapping_add(2u64 ^ (XXPRIME_5 ^ 3527539));
        if acc == u64::MAX {
            return 1546275796;
        }
        acc as i64
    }
    #[cfg(target_pointer_width = "32")]
    {
        const XXPRIME_1: u32 = 2654435761;
        const XXPRIME_2: u32 = 2246822519;
        const XXPRIME_5: u32 = 374761393;
        let mut acc = XXPRIME_5;
        for lane_bits in [origin_bits, args_bits] {
            let lane = hash_bits_signed(_py, lane_bits);
            if exception_pending(_py) {
                return 0;
            }
            acc = acc.wrapping_add((lane as u32).wrapping_mul(XXPRIME_2));
            acc = acc.rotate_left(13);
            acc = acc.wrapping_mul(XXPRIME_1);
        }
        acc = acc.wrapping_add(2u32 ^ (XXPRIME_5 ^ 3527539));
        if acc == u32::MAX {
            return 1546275796;
        }
        (acc as i32) as i64
    }
}

fn hash_union_type(_py: &PyToken<'_>, ptr: *mut u8) -> i64 {
    let args_bits = unsafe { union_type_args_bits(ptr) };
    #[cfg(target_pointer_width = "64")]
    {
        const XXPRIME_1: u64 = 11400714785074694791;
        const XXPRIME_2: u64 = 14029467366897019727;
        const XXPRIME_5: u64 = 2870177450012600261;
        let lane = hash_bits_signed(_py, args_bits);
        if exception_pending(_py) {
            return 0;
        }
        let mut acc = XXPRIME_5;
        acc = acc.wrapping_add((lane as u64).wrapping_mul(XXPRIME_2));
        acc = acc.rotate_left(31);
        acc = acc.wrapping_mul(XXPRIME_1);
        acc = acc.wrapping_add(1u64 ^ (XXPRIME_5 ^ 3527539));
        if acc == u64::MAX {
            return 1546275796;
        }
        acc as i64
    }
    #[cfg(target_pointer_width = "32")]
    {
        const XXPRIME_1: u32 = 2654435761;
        const XXPRIME_2: u32 = 2246822519;
        const XXPRIME_5: u32 = 374761393;
        let lane = hash_bits_signed(_py, args_bits);
        if exception_pending(_py) {
            return 0;
        }
        let mut acc = XXPRIME_5;
        acc = acc.wrapping_add((lane as u32).wrapping_mul(XXPRIME_2));
        acc = acc.rotate_left(13);
        acc = acc.wrapping_mul(XXPRIME_1);
        acc = acc.wrapping_add(1u32 ^ (XXPRIME_5 ^ 3527539));
        if acc == u32::MAX {
            return 1546275796;
        }
        (acc as i32) as i64
    }
}

#[cfg(target_pointer_width = "64")]
fn slice_hash_acc(lanes: [u64; 3]) -> u64 {
    const XXPRIME_1: u64 = 11400714785074694791;
    const XXPRIME_2: u64 = 14029467366897019727;
    const XXPRIME_5: u64 = 2870177450012600261;
    let mut acc = XXPRIME_5;
    for lane in lanes {
        acc = acc.wrapping_add(lane.wrapping_mul(XXPRIME_2));
        acc = acc.rotate_left(31);
        acc = acc.wrapping_mul(XXPRIME_1);
    }
    acc
}

#[cfg(target_pointer_width = "32")]
fn slice_hash_acc(lanes: [u32; 3]) -> u32 {
    const XXPRIME_1: u32 = 2654435761;
    const XXPRIME_2: u32 = 2246822519;
    const XXPRIME_5: u32 = 374761393;
    let mut acc = XXPRIME_5;
    for lane in lanes {
        acc = acc.wrapping_add(lane.wrapping_mul(XXPRIME_2));
        acc = acc.rotate_left(13);
        acc = acc.wrapping_mul(XXPRIME_1);
    }
    acc
}

pub(crate) fn hash_slice_bits(
    _py: &PyToken<'_>,
    start_bits: u64,
    stop_bits: u64,
    step_bits: u64,
) -> Option<i64> {
    let mut lanes = [0i64; 3];
    let elems = [start_bits, stop_bits, step_bits];
    for (idx, bits) in elems.iter().enumerate() {
        lanes[idx] = hash_bits_signed(_py, *bits);
        if exception_pending(_py) {
            return None;
        }
    }
    #[cfg(target_pointer_width = "64")]
    {
        let acc = slice_hash_acc([lanes[0] as u64, lanes[1] as u64, lanes[2] as u64]);
        if acc == u64::MAX {
            return Some(1546275796);
        }
        Some(acc as i64)
    }
    #[cfg(target_pointer_width = "32")]
    {
        let acc = slice_hash_acc([lanes[0] as u32, lanes[1] as u32, lanes[2] as u32]);
        if acc == u32::MAX {
            return Some(1546275796);
        }
        Some((acc as i32) as i64)
    }
}

fn shuffle_frozenset_hash(hash: u64) -> u64 {
    let mixed = (hash ^ 89869747u64) ^ (hash << 16);
    mixed.wrapping_mul(3644798167u64)
}

fn hash_frozenset(_py: &PyToken<'_>, ptr: *mut u8) -> i64 {
    let elems = unsafe { set_order(ptr) };
    let mut hash = 0u64;
    for &elem in elems.iter() {
        hash ^= shuffle_frozenset_hash(hash_bits(_py, elem));
    }
    if elems.len() & 1 == 1 {
        hash ^= shuffle_frozenset_hash(0);
    }
    hash ^= ((elems.len() as u64) + 1).wrapping_mul(1927868237u64);
    hash ^= (hash >> 11) ^ (hash >> 25);
    hash = hash.wrapping_mul(69069u64).wrapping_add(907133923u64);
    if hash == u64::MAX {
        hash = 590923713u64;
    }
    hash as i64
}

pub(crate) fn hash_pointer(ptr: u64) -> i64 {
    let hash = (ptr >> 4) as i64;
    fix_hash(hash)
}

fn hash_unhashable(_py: &PyToken<'_>, obj: MoltObject) -> i64 {
    let name = type_name(_py, obj);
    let msg = format!("unhashable type: '{name}'");
    raise_exception::<_>(_py, "TypeError", &msg)
}

fn is_unhashable_type(type_id: u32) -> bool {
    matches!(
        type_id,
        TYPE_ID_LIST
            | TYPE_ID_DICT
            | TYPE_ID_SET
            | TYPE_ID_BYTEARRAY
            | TYPE_ID_MEMORYVIEW
            | TYPE_ID_LIST_BUILDER
            | TYPE_ID_DICT_BUILDER
            | TYPE_ID_SET_BUILDER
            | TYPE_ID_DICT_KEYS_VIEW
            | TYPE_ID_DICT_VALUES_VIEW
            | TYPE_ID_DICT_ITEMS_VIEW
            | TYPE_ID_CALLARGS
    )
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
        return PY_HASH_NONE;
    }
    if let Some(f) = obj.as_float() {
        return hash_float(f);
    }
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if is_unhashable_type(type_id) {
                return hash_unhashable(_py, obj);
            }
            if type_id == TYPE_ID_STRING {
                return hash_string(_py, ptr);
            }
            if type_id == TYPE_ID_BYTES || type_id == TYPE_ID_BYTEARRAY {
                let len = bytes_len(ptr);
                let bytes = std::slice::from_raw_parts(bytes_data(ptr), len);
                return hash_bytes_cached(_py, ptr, bytes);
            }
            if type_id == TYPE_ID_BIGINT {
                return hash_bigint(ptr);
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
                        let hash_name_bits = intern_static_name(
                            _py,
                            &runtime_state(_py).interned.hash_name,
                            b"__hash__",
                        );
                        if let Some(call_bits) =
                            attr_lookup_ptr_allow_missing(_py, ptr, hash_name_bits)
                        {
                            let res_bits = call_callable0(_py, call_bits);
                            dec_ref_bits(_py, call_bits);
                            if exception_pending(_py) {
                                dec_ref_bits(_py, res_bits);
                                return 0;
                            }
                            let res_obj = obj_from_bits(res_bits);
                            if let Some(val) = to_i64(res_obj) {
                                dec_ref_bits(_py, res_bits);
                                return fix_hash(val);
                            }
                            if let Some(big_ptr) = bigint_ptr_from_bits(res_bits) {
                                let big = bigint_ref(big_ptr);
                                let Some(val) = big.to_i64() else {
                                    dec_ref_bits(_py, res_bits);
                                    return raise_exception::<i64>(
                                        _py,
                                        "OverflowError",
                                        "cannot fit 'int' into an index-sized integer",
                                    );
                                };
                                dec_ref_bits(_py, res_bits);
                                return fix_hash(val);
                            }
                            dec_ref_bits(_py, res_bits);
                            return raise_exception::<i64>(
                                _py,
                                "TypeError",
                                "__hash__ returned non-int",
                            );
                        }
                        return hash_pointer(ptr as u64);
                    }
                    1 => {
                        let fields = dataclass_fields_ref(ptr);
                        let type_label = if desc.name.is_empty() {
                            "dataclass"
                        } else {
                            desc.name.as_str()
                        };
                        return hash_dataclass_fields(
                            _py,
                            fields,
                            &desc.field_flags,
                            &desc.field_names,
                            type_label,
                        );
                    }
                    _ => return hash_pointer(ptr as u64),
                }
            }
            if type_id == TYPE_ID_TYPE {
                let metaclass_bits = type_of_bits(_py, obj.bits());
                if metaclass_bits == builtin_classes(_py).type_obj {
                    return hash_pointer(ptr as u64);
                }
                let hash_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.hash_name, b"__hash__");
                let eq_name_bits =
                    intern_static_name(_py, &runtime_state(_py).interned.eq_name, b"__eq__");
                let mut meta_overrides_hash = false;
                if let Some(meta_ptr) = obj_from_bits(metaclass_bits).as_ptr()
                    && object_type_id(meta_ptr) == TYPE_ID_TYPE
                {
                    let dict_bits = class_dict_bits(meta_ptr);
                    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                        && object_type_id(dict_ptr) == TYPE_ID_DICT
                    {
                        meta_overrides_hash = dict_get_in_place(_py, dict_ptr, hash_name_bits)
                            .is_some()
                            || dict_get_in_place(_py, dict_ptr, eq_name_bits).is_some();
                    }
                }
                if meta_overrides_hash && let Some(hash) = hash_from_dunder(_py, obj, ptr) {
                    return hash;
                }
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
        let eq_name_bits = intern_static_name(_py, &runtime_state(_py).interned.eq_name, b"__eq__");
        let class_bits = type_of_bits(_py, obj.bits());
        let default_type_hashable = class_bits == builtin_classes(_py).type_obj;
        if let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
            && object_type_id(class_ptr) == TYPE_ID_TYPE
            && !default_type_hashable
        {
            let dict_bits = class_dict_bits(class_ptr);
            if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
                && object_type_id(dict_ptr) == TYPE_ID_DICT
            {
                let hash_entry = dict_get_in_place(_py, dict_ptr, hash_name_bits);
                if exception_pending(_py) {
                    return Some(0);
                }
                if let Some(hash_bits) = hash_entry {
                    if obj_from_bits(hash_bits).is_none() {
                        let name = type_name(_py, obj);
                        let msg = format!("unhashable type: '{name}'");
                        return Some(raise_exception::<i64>(_py, "TypeError", &msg));
                    }
                } else if dict_get_in_place(_py, dict_ptr, eq_name_bits).is_some() {
                    let name = type_name(_py, obj);
                    let msg = format!("unhashable type: '{name}'");
                    return Some(raise_exception::<i64>(_py, "TypeError", &msg));
                }
                if exception_pending(_py) {
                    return Some(0);
                }
            }
        }
        let call_bits = attr_lookup_ptr_allow_missing(_py, obj_ptr, hash_name_bits)?;
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
        let res_obj = obj_from_bits(res_bits);
        let hash = if let Some(i) = to_i64(res_obj) {
            hash_int(i)
        } else if let Some(ptr) = res_obj.as_ptr() {
            if object_type_id(ptr) == TYPE_ID_BIGINT {
                hash_bigint(ptr)
            } else {
                let msg = "__hash__ method should return an integer";
                dec_ref_bits(_py, res_bits);
                return Some(raise_exception::<i64>(_py, "TypeError", msg));
            }
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

pub(crate) fn ensure_hashable(_py: &PyToken<'_>, key_bits: u64) -> bool {
    let obj = obj_from_bits(key_bits);
    if let Some(ptr) = obj.as_ptr() {
        unsafe {
            let type_id = object_type_id(ptr);
            if is_unhashable_type(type_id) {
                let name = type_name(_py, obj);
                let msg = format!("unhashable type: '{name}'");
                return raise_exception::<_>(_py, "TypeError", &msg);
            }
        }
    }
    true
}
