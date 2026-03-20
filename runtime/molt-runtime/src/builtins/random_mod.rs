// === FILE: runtime/molt-runtime/src/builtins/random_mod.rs ===
//
// Mersenne Twister random module intrinsics for Molt.
//
// Mirrors CPython's `random` module (Lib/random.py, Modules/_randommodule.c),
// implemented entirely in Rust with no CPython fallback.
//
// Handle model: global Mutex<HashMap<i64, MersenneTwisterRng>> keyed by an
// atomically-issued handle ID, returned to Python as a NaN-boxed integer.
// Uses a global registry (not thread-local) so handles are visible across all
// threads. The GIL serializes all Python-level access, so the Mutex is always
// uncontended.
//
// MT constants follow the original Matsumoto & Nishimura 1998 parameters.
// Distribution algorithms follow CPython 3.12 random.py exactly.

use crate::randomness::fill_os_random;
use crate::*;
#[cfg(feature = "stdlib_crypto")]
use digest::Digest;
use num_bigint::{BigInt, BigUint, Sign};
use num_integer::Integer;
use num_traits::{One, Signed, ToPrimitive, Zero};
#[cfg(feature = "stdlib_crypto")]
use sha2::Sha512;
use std::collections::HashMap;
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{LazyLock, Mutex};

// ─── Mersenne Twister constants ───────────────────────────────────────────────

const MT_N: usize = 624;
const MT_M: usize = 397;
const MT_MATRIX_A: u32 = 0x9908_B0DF;
const MT_UPPER_MASK: u32 = 0x8000_0000;
const MT_LOWER_MASK: u32 = 0x7FFF_FFFF;
// 1 / 2^53 — converts 53-bit integer to [0, 1) float
const MT_RECIP_53: f64 = 1.0 / 9_007_199_254_740_992.0;

// ─── Math helpers (private, platform-specific, mirrors math.rs pattern) ───────

#[cfg(target_arch = "wasm32")]
#[inline(always)]
fn rng_sqrt(x: f64) -> f64 {
    libm::sqrt(x)
}
#[cfg(not(target_arch = "wasm32"))]
#[inline(always)]
fn rng_sqrt(x: f64) -> f64 {
    x.sqrt()
}

#[cfg(target_arch = "wasm32")]
#[inline(always)]
fn rng_log(x: f64) -> f64 {
    libm::log(x)
}
#[cfg(not(target_arch = "wasm32"))]
#[inline(always)]
fn rng_log(x: f64) -> f64 {
    x.ln()
}

#[cfg(target_arch = "wasm32")]
#[allow(dead_code)]
#[inline(always)]
fn rng_log2(x: f64) -> f64 {
    libm::log2(x)
}
#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
#[inline(always)]
fn rng_log2(x: f64) -> f64 {
    x.log2()
}

#[cfg(target_arch = "wasm32")]
#[inline(always)]
fn rng_exp(x: f64) -> f64 {
    libm::exp(x)
}
#[cfg(not(target_arch = "wasm32"))]
#[inline(always)]
fn rng_exp(x: f64) -> f64 {
    x.exp()
}

#[cfg(target_arch = "wasm32")]
#[inline(always)]
fn rng_cos(x: f64) -> f64 {
    libm::cos(x)
}
#[cfg(not(target_arch = "wasm32"))]
#[inline(always)]
fn rng_cos(x: f64) -> f64 {
    x.cos()
}

#[cfg(target_arch = "wasm32")]
#[inline(always)]
fn rng_sin(x: f64) -> f64 {
    libm::sin(x)
}
#[cfg(not(target_arch = "wasm32"))]
#[inline(always)]
fn rng_sin(x: f64) -> f64 {
    x.sin()
}

#[cfg(target_arch = "wasm32")]
#[inline(always)]
fn rng_atan(x: f64) -> f64 {
    libm::atan(x)
}
#[cfg(not(target_arch = "wasm32"))]
#[inline(always)]
fn rng_atan(x: f64) -> f64 {
    x.atan()
}

#[cfg(target_arch = "wasm32")]
#[allow(dead_code)]
#[inline(always)]
fn rng_fabs(x: f64) -> f64 {
    libm::fabs(x)
}
#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
#[inline(always)]
fn rng_fabs(x: f64) -> f64 {
    x.abs()
}

#[cfg(target_arch = "wasm32")]
#[allow(dead_code)]
#[inline(always)]
fn rng_floor(x: f64) -> f64 {
    libm::floor(x)
}
#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)]
#[inline(always)]
fn rng_floor(x: f64) -> f64 {
    x.floor()
}

// ─── Handle counter ───────────────────────────────────────────────────────────

static NEXT_RANDOM_HANDLE: AtomicI64 = AtomicI64::new(1);

fn next_random_handle() -> i64 {
    NEXT_RANDOM_HANDLE.fetch_add(1, Ordering::Relaxed)
}

// ─── MersenneTwisterRng struct ────────────────────────────────────────────────
//
// Adapted from StatisticsRandomRng in math.rs (lines 2913-3038).
// Added `gauss_next` for Box-Muller caching (same as CPython random.py).

#[derive(Clone)]
struct MersenneTwisterRng {
    mt: [u32; MT_N],
    index: usize,
    /// Cached second Gaussian sample (Box-Muller generates two at a time).
    gauss_next: Option<f64>,
}

impl MersenneTwisterRng {
    fn new_from_seed_key(seed_key: &[u32]) -> Self {
        let mut rng = Self {
            mt: [0u32; MT_N],
            index: MT_N,
            gauss_next: None,
        };
        rng.init_by_array(seed_key);
        rng.index = MT_N;
        rng.gauss_next = None;
        rng
    }

    fn init_genrand(&mut self, seed: u32) {
        self.mt[0] = seed;
        for i in 1..MT_N {
            let prev = self.mt[i - 1];
            self.mt[i] = 1_812_433_253u32
                .wrapping_mul(prev ^ (prev >> 30))
                .wrapping_add(i as u32);
        }
    }

    fn init_by_array(&mut self, init_key: &[u32]) {
        self.init_genrand(19_650_218);
        let mut i = 1usize;
        let mut j = 0usize;
        let key_length = init_key.len();
        let k = MT_N.max(key_length);

        for _ in 0..k {
            let prev = self.mt[i - 1];
            let mixed = self.mt[i] ^ ((prev ^ (prev >> 30)).wrapping_mul(1_664_525));
            self.mt[i] = mixed.wrapping_add(init_key[j]).wrapping_add(j as u32);
            i += 1;
            j += 1;
            if i >= MT_N {
                self.mt[0] = self.mt[MT_N - 1];
                i = 1;
            }
            if j >= key_length {
                j = 0;
            }
        }

        for _ in 0..(MT_N - 1) {
            let prev = self.mt[i - 1];
            let mixed = self.mt[i] ^ ((prev ^ (prev >> 30)).wrapping_mul(1_566_083_941));
            self.mt[i] = mixed.wrapping_sub(i as u32);
            i += 1;
            if i >= MT_N {
                self.mt[0] = self.mt[MT_N - 1];
                i = 1;
            }
        }

        self.mt[0] = MT_UPPER_MASK;
    }

    fn twist(&mut self) {
        for i in 0..MT_N {
            let y = (self.mt[i] & MT_UPPER_MASK) | (self.mt[(i + 1) % MT_N] & MT_LOWER_MASK);
            let mut value = self.mt[(i + MT_M) % MT_N] ^ (y >> 1);
            if y & 1 != 0 {
                value ^= MT_MATRIX_A;
            }
            self.mt[i] = value;
        }
        self.index = 0;
    }

    fn rand_u32(&mut self) -> u32 {
        if self.index >= MT_N {
            self.twist();
        }
        let mut y = self.mt[self.index];
        self.index += 1;
        // Tempering
        y ^= y >> 11;
        y ^= (y << 7) & 0x9D2C_5680;
        y ^= (y << 15) & 0xEFC6_0000;
        y ^= y >> 18;
        y
    }

    /// Generate a double in [0.0, 1.0) using 53 bits of randomness.
    fn random(&mut self) -> f64 {
        let a = (self.rand_u32() >> 5) as u64;
        let b = (self.rand_u32() >> 6) as u64;
        (a as f64 * 67_108_864.0 + b as f64) * MT_RECIP_53
    }

    /// Box-Muller Gaussian with caching (CPython random.gauss).
    fn gauss(&mut self, mu: f64, sigma: f64) -> f64 {
        let z = if let Some(next) = self.gauss_next.take() {
            next
        } else {
            let x2pi = self.random() * core::f64::consts::TAU;
            let g2rad = rng_sqrt(-2.0 * rng_log(1.0 - self.random()));
            let z = rng_cos(x2pi) * g2rad;
            self.gauss_next = Some(rng_sin(x2pi) * g2rad);
            z
        };
        mu + z * sigma
    }

    /// Kinderman-Monahan normal variate (CPython random.normalvariate).
    fn normalvariate(&mut self, mu: f64, sigma: f64) -> f64 {
        // Uses Kinderman-Monahan method (see Knuth TAOCP Vol. 2, §3.4.1 C)
        const NV_MAGICCONST: f64 = 4.0 * 0.606_530_659_712_633_4; // 4 * exp(-0.5) / sqrt(2.0)
        loop {
            let u1 = self.random();
            let u2 = 1.0 - self.random();
            let z = NV_MAGICCONST * (u1 - 0.5) / u2;
            let zz = z * z / 4.0;
            if zz <= -rng_log(u2) {
                return mu + z * sigma;
            }
        }
    }

    /// Generate k random bits as a BigUint.
    fn randbits_biguint(&mut self, k: u32) -> BigUint {
        if k == 0 {
            return BigUint::zero();
        }
        let words_needed = (k as usize).div_ceil(32);
        let mut words: Vec<u32> = (0..words_needed).map(|_| self.rand_u32()).collect();
        // Mask the top word to exactly k bits
        let remainder = k % 32;
        if remainder != 0 {
            *words.last_mut().unwrap() &= (1u32 << remainder) - 1;
        }
        // BigUint::new expects little-endian u32 digits
        BigUint::new(words)
    }

    /// Rejection-sampled randbelow(n) — CPython _randbelow_with_getrandbits.
    fn randbelow(&mut self, n: &BigInt) -> BigInt {
        if n.is_zero() {
            return BigInt::zero();
        }
        let abs_n = n.abs().to_biguint().unwrap();
        let k = abs_n.bits() as u32;
        if k == 0 {
            return BigInt::zero();
        }
        loop {
            let r = self.randbits_biguint(k);
            if r < abs_n {
                return BigInt::from(r);
            }
        }
    }
}

// ─── Global registry ──────────────────────────────────────────────────────────

static RANDOM_REGISTRY: LazyLock<Mutex<HashMap<i64, MersenneTwisterRng>>> =
    LazyLock::new(|| Mutex::new(HashMap::new()));

// ─── Internal helpers ─────────────────────────────────────────────────────────

fn rng_handle_from_bits(_py: &PyToken<'_>, handle_bits: u64) -> Option<i64> {
    let obj = obj_from_bits(handle_bits);
    let Some(id) = to_i64(obj) else {
        let _ = raise_exception::<u64>(_py, "TypeError", "Random handle must be an int");
        return None;
    };
    Some(id)
}

/// Extract an f64 from NaN-boxed bits (float or int). Sets exception and returns None on error.
fn f64_from_bits(_py: &PyToken<'_>, bits: u64, param_name: &str) -> Option<f64> {
    let obj = obj_from_bits(bits);
    if let Some(f) = obj.as_float() {
        return Some(f);
    }
    if let Some(i) = to_i64(obj) {
        return Some(i as f64);
    }
    if let Some(ptr) = bigint_ptr_from_bits(bits) {
        let big = unsafe { bigint_ref(ptr) };
        return Some(big.to_f64().unwrap_or(f64::INFINITY));
    }
    let _ = raise_exception::<u64>(_py, "TypeError", &format!("{param_name} must be a number"));
    None
}

/// Seed the RNG from a BigInt (decompose into u32 words).
fn seed_key_from_bigint(seed: &BigInt) -> Vec<u32> {
    let mut key = seed
        .abs()
        .to_biguint()
        .map_or_else(Vec::new, |v| v.to_u32_digits());
    if key.is_empty() {
        key.push(0);
    }
    key
}

/// Seed from system time via getrandom — used when seed is None.
fn seed_from_os(_py: &PyToken<'_>) -> Option<Vec<u32>> {
    let mut buf = [0u8; 32];
    fill_os_random(&mut buf)
        .map_err(|_| {
            let _ = raise_exception::<u64>(_py, "OSError", "getrandom failed");
        })
        .ok()?;
    let big = BigInt::from_bytes_be(Sign::Plus, &buf);
    let key = seed_key_from_bigint(&big);
    Some(key)
}

/// Extract a BigInt seed from a Python object, applying CPython version=2 hashing rules.
/// Returns None and sets exception on type error.
fn seed_bigint_from_bits(_py: &PyToken<'_>, seed_bits: u64) -> Option<BigInt> {
    let seed_obj = obj_from_bits(seed_bits);

    // None → caller should use os random
    if seed_obj.is_none() {
        return None;
    }

    // int (inline small)
    if let Some(i) = to_i64(seed_obj) {
        return Some(BigInt::from(i).abs());
    }

    // bigint heap
    if let Some(ptr) = bigint_ptr_from_bits(seed_bits) {
        return Some(unsafe { bigint_ref(ptr).abs() });
    }

    // float → hash it
    if seed_obj.as_float().is_some() {
        let hash_bits = crate::molt_hash_builtin(seed_bits);
        if exception_pending(_py) {
            return None;
        }
        let hash_obj = obj_from_bits(hash_bits);
        let hash_u64 = if let Some(i) = to_i64(hash_obj) {
            i as u64
        } else if let Some(ptr) = bigint_ptr_from_bits(hash_bits) {
            let hash_big = unsafe { bigint_ref(ptr).clone() };
            let modulus = BigInt::one() << 64;
            hash_big
                .mod_floor(&modulus)
                .to_biguint()
                .and_then(|v| v.to_u64())
                .unwrap_or(0)
        } else {
            if maybe_ptr_from_bits(hash_bits).is_some() {
                dec_ref_bits(_py, hash_bits);
            }
            let _ = raise_exception::<u64>(_py, "TypeError", "hash() should return an integer");
            return None;
        };
        if maybe_ptr_from_bits(hash_bits).is_some() {
            dec_ref_bits(_py, hash_bits);
        }
        return Some(BigInt::from(hash_u64));
    }

    // str / bytes / bytearray → SHA-512 then encode as big integer (version=2 semantics)
    let Some(seed_ptr) = seed_obj.as_ptr() else {
        let _ = raise_exception::<u64>(
            _py,
            "TypeError",
            "The only supported seed types are: None, int, float, str, bytes, bytearray.",
        );
        return None;
    };

    let seed_bytes: Vec<u8> = unsafe {
        match object_type_id(seed_ptr) {
            TYPE_ID_STRING => {
                std::slice::from_raw_parts(string_bytes(seed_ptr), string_len(seed_ptr)).to_vec()
            }
            TYPE_ID_BYTES | TYPE_ID_BYTEARRAY => {
                let Some(slice) = bytes_like_slice(seed_ptr) else {
                    let _ = raise_exception::<u64>(
                        _py,
                        "TypeError",
                        "The only supported seed types are: None, int, float, str, bytes, bytearray.",
                    );
                    return None;
                };
                slice.to_vec()
            }
            _ => {
                let _ = raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "The only supported seed types are: None, int, float, str, bytes, bytearray.",
                );
                return None;
            }
        }
    };

    #[cfg(feature = "stdlib_crypto")]
    {
        let digest = Sha512::digest(&seed_bytes);
        let mut payload = Vec::with_capacity(seed_bytes.len() + digest.len());
        payload.extend_from_slice(&seed_bytes);
        payload.extend_from_slice(&digest);
        Some(BigInt::from(BigUint::from_bytes_be(&payload)))
    }
    #[cfg(not(feature = "stdlib_crypto"))]
    {
        // Without crypto support, fall back to using the raw seed bytes.
        Some(BigInt::from(BigUint::from_bytes_be(&seed_bytes)))
    }
}

// ─── Allocation helpers ───────────────────────────────────────────────────────

fn alloc_tuple_bits(_py: &PyToken<'_>, elems: &[u64]) -> Result<u64, u64> {
    let ptr = alloc_tuple(_py, elems);
    if ptr.is_null() {
        return Err(raise_exception::<u64>(_py, "MemoryError", "out of memory"));
    }
    Ok(MoltObject::from_ptr(ptr).bits())
}

fn alloc_list_bits(_py: &PyToken<'_>, elems: &[u64]) -> Result<u64, u64> {
    let ptr = alloc_list(_py, elems);
    if ptr.is_null() {
        return Err(raise_exception::<u64>(_py, "MemoryError", "out of memory"));
    }
    Ok(MoltObject::from_ptr(ptr).bits())
}

// ─── Public intrinsics ────────────────────────────────────────────────────────

/// Create a new MT RNG seeded from OS randomness. Returns integer handle.
#[unsafe(no_mangle)]
pub extern "C" fn molt_random_new() -> u64 {
    crate::with_gil_entry!(_py, {
        let key = match seed_from_os(_py) {
            Some(k) => k,
            None => {
                // Fallback: single-word 0 seed
                vec![0u32]
            }
        };
        let rng = MersenneTwisterRng::new_from_seed_key(&key);
        let id = next_random_handle();
        RANDOM_REGISTRY.lock().unwrap().insert(id, rng);
        MoltObject::from_int(id).bits()
    })
}

/// Seed the RNG.
///
/// `version_bits` — integer, 1 or 2 (Molt always uses version=2 semantics for
///                  str/bytes, matching CPython 3.2+).
/// `seed_bits`    — None, int, float, str, bytes, or bytearray.
///                  None → re-seed from OS.
#[unsafe(no_mangle)]
pub extern "C" fn molt_random_seed(handle_bits: u64, seed_bits: u64, _version_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = rng_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };

        let seed_obj = obj_from_bits(seed_bits);
        let key = if seed_obj.is_none() {
            // None → OS random
            match seed_from_os(_py) {
                Some(k) => k,
                None => return MoltObject::none().bits(),
            }
        } else {
            match seed_bigint_from_bits(_py, seed_bits) {
                Some(big) => seed_key_from_bigint(&big),
                None => {
                    if exception_pending(_py) {
                        return MoltObject::none().bits();
                    }
                    // Should not happen, but guard anyway
                    return MoltObject::none().bits();
                }
            }
        };

        let new_rng = MersenneTwisterRng::new_from_seed_key(&key);
        if let Some(entry) = RANDOM_REGISTRY.lock().unwrap().get_mut(&id) {
            *entry = new_rng;
        }
        MoltObject::none().bits()
    })
}

/// Return a random float in [0.0, 1.0).
#[unsafe(no_mangle)]
pub extern "C" fn molt_random_random(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = rng_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let mut reg = RANDOM_REGISTRY.lock().unwrap();
        let Some(rng) = reg.get_mut(&id) else {
            return raise_exception::<u64>(_py, "ValueError", "invalid Random handle");
        };
        MoltObject::from_float(rng.random()).bits()
    })
}

/// Return a Python int with k random bits.
#[unsafe(no_mangle)]
pub extern "C" fn molt_random_getrandbits(handle_bits: u64, k_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = rng_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let k_obj = obj_from_bits(k_bits);
        let Some(k_i64) = to_i64(k_obj) else {
            return raise_exception::<u64>(_py, "TypeError", "k must be an integer");
        };
        if k_i64 < 0 {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "number of bits must be non-negative",
            );
        }
        if k_i64 > 65_536 {
            return raise_exception::<u64>(_py, "ValueError", "number of bits must be <= 65536");
        }
        let k = k_i64 as u32;
        if k == 0 {
            return int_bits_from_i64(_py, 0);
        }
        let big = {
            let mut reg = RANDOM_REGISTRY.lock().unwrap();
            let Some(rng) = reg.get_mut(&id) else {
                return raise_exception::<u64>(_py, "ValueError", "invalid Random handle");
            };
            rng.randbits_biguint(k)
        };
        // Try to fit in i64 first to avoid heap allocation
        if let Some(small) = big.to_u64()
            && small <= i64::MAX as u64
        {
            return int_bits_from_i64(_py, small as i64);
        }
        int_bits_from_bigint(_py, BigInt::from(big))
    })
}

/// Return a random integer in [0, n) via rejection sampling.
/// `n_bits` must be a Python int.
#[unsafe(no_mangle)]
pub extern "C" fn molt_random_randbelow(handle_bits: u64, n_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = rng_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };

        let n_obj = obj_from_bits(n_bits);
        let n_big: BigInt = if let Some(i) = to_i64(n_obj) {
            BigInt::from(i)
        } else if let Some(ptr) = bigint_ptr_from_bits(n_bits) {
            unsafe { bigint_ref(ptr).clone() }
        } else {
            return raise_exception::<u64>(_py, "TypeError", "n must be an integer");
        };

        if n_big <= BigInt::zero() {
            return raise_exception::<u64>(_py, "ValueError", "n must be positive");
        }

        let result = {
            let mut reg = RANDOM_REGISTRY.lock().unwrap();
            let Some(rng) = reg.get_mut(&id) else {
                return raise_exception::<u64>(_py, "ValueError", "invalid Random handle");
            };
            rng.randbelow(&n_big)
        };

        if let Some(small) = result.to_i64() {
            int_bits_from_i64(_py, small)
        } else {
            int_bits_from_bigint(_py, result)
        }
    })
}

/// Get the internal MT state as a Python tuple:
///   (version=3, internalstate_tuple, gauss_next_or_None)
/// where internalstate_tuple is a 625-element tuple of u32 words (624 MT state + index).
#[unsafe(no_mangle)]
pub extern "C" fn molt_random_getstate(handle_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = rng_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };

        let (mt_words, index, gauss_next) = {
            let reg = RANDOM_REGISTRY.lock().unwrap();
            let Some(rng) = reg.get(&id) else {
                return raise_exception::<u64>(_py, "ValueError", "invalid Random handle");
            };
            (rng.mt, rng.index, rng.gauss_next)
        };

        // Build 625-element internalstate tuple: [mt[0]..mt[623], index]
        let mut state_elems: Vec<u64> = Vec::with_capacity(MT_N + 1);
        for &word in &mt_words {
            state_elems.push(MoltObject::from_int(word as i64).bits());
        }
        state_elems.push(MoltObject::from_int(index as i64).bits());

        let internalstate_bits = match alloc_tuple_bits(_py, &state_elems) {
            Ok(b) => b,
            Err(exc) => return exc,
        };

        let gauss_bits = match gauss_next {
            Some(f) => MoltObject::from_float(f).bits(),
            None => MoltObject::none().bits(),
        };

        let version_bits = MoltObject::from_int(3).bits();
        let outer = match alloc_tuple_bits(_py, &[version_bits, internalstate_bits, gauss_bits]) {
            Ok(b) => b,
            Err(exc) => {
                dec_ref_bits(_py, internalstate_bits);
                return exc;
            }
        };
        // dec_ref our local hold on internalstate_bits (outer tuple now owns it)
        dec_ref_bits(_py, internalstate_bits);
        outer
    })
}

/// Restore the MT state from a tuple produced by getstate().
/// Expected format: (version, internalstate_tuple, gauss_next_or_None)
#[unsafe(no_mangle)]
pub extern "C" fn molt_random_setstate(handle_bits: u64, state_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = rng_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };

        let state_obj = obj_from_bits(state_bits);
        let Some(state_ptr) = state_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "state must be a tuple");
        };
        let state_type = unsafe { object_type_id(state_ptr) };
        if state_type != TYPE_ID_TUPLE && state_type != TYPE_ID_LIST {
            return raise_exception::<u64>(_py, "TypeError", "state must be a tuple");
        }

        let outer_elems = unsafe { seq_vec_ref(state_ptr) };
        if outer_elems.len() < 3 {
            return raise_exception::<u64>(_py, "ValueError", "state tuple must have 3 elements");
        }

        // element 1: internalstate tuple (625 elements)
        let inner_bits = outer_elems[1];
        let inner_obj = obj_from_bits(inner_bits);
        let Some(inner_ptr) = inner_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "internalstate must be a tuple");
        };
        let inner_type = unsafe { object_type_id(inner_ptr) };
        if inner_type != TYPE_ID_TUPLE && inner_type != TYPE_ID_LIST {
            return raise_exception::<u64>(_py, "TypeError", "internalstate must be a tuple");
        }
        let inner_elems = unsafe { seq_vec_ref(inner_ptr) };
        if inner_elems.len() != MT_N + 1 {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "internalstate must have 625 elements",
            );
        }

        let mut mt = [0u32; MT_N];
        for (i, &bits) in inner_elems[..MT_N].iter().enumerate() {
            let obj = obj_from_bits(bits);
            let Some(v) = to_i64(obj) else {
                return raise_exception::<u64>(_py, "TypeError", "MT state elements must be ints");
            };
            mt[i] = v as u32;
        }
        let index_obj = obj_from_bits(inner_elems[MT_N]);
        let Some(index_i64) = to_i64(index_obj) else {
            return raise_exception::<u64>(_py, "TypeError", "MT index must be an int");
        };
        let index = (index_i64 as usize).min(MT_N);

        // element 2: gauss_next (float or None)
        let gauss_bits = outer_elems[2];
        let gauss_obj = obj_from_bits(gauss_bits);
        let gauss_next = if gauss_obj.is_none() {
            None
        } else if let Some(f) = gauss_obj.as_float() {
            Some(f)
        } else {
            to_i64(gauss_obj).map(|i| i as f64)
        };

        {
            let mut reg = RANDOM_REGISTRY.lock().unwrap();
            let Some(rng) = reg.get_mut(&id) else {
                return raise_exception::<u64>(_py, "ValueError", "invalid Random handle");
            };
            rng.mt = mt;
            rng.index = index;
            rng.gauss_next = gauss_next;
        }

        MoltObject::none().bits()
    })
}

/// Fisher-Yates in-place shuffle of a Molt list.
/// `list_bits` must be a mutable list object.
#[unsafe(no_mangle)]
pub extern "C" fn molt_random_shuffle(handle_bits: u64, list_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = rng_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };

        let list_obj = obj_from_bits(list_bits);
        let Some(list_ptr) = list_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "shuffle requires a list");
        };
        let list_type = unsafe { object_type_id(list_ptr) };
        if list_type != TYPE_ID_LIST {
            return raise_exception::<u64>(_py, "TypeError", "shuffle requires a list");
        }

        let n = unsafe { list_len(list_ptr) };
        if n < 2 {
            return MoltObject::none().bits();
        }

        // We need mutable access to the list's Vec and the RNG simultaneously.
        // Do rejection sampling outside the lock, then apply inside — but since
        // the GIL serializes us, we can just lock once and work.
        let mut reg = RANDOM_REGISTRY.lock().unwrap();
        let Some(rng) = reg.get_mut(&id) else {
            return raise_exception::<u64>(_py, "ValueError", "invalid Random handle");
        };

        // Fisher-Yates: for i from n-1 down to 1, swap list[i] with list[randbelow(i+1)]
        let vec = unsafe { seq_vec(list_ptr) };
        for i in (1..n).rev() {
            // Inline rejection sampling for [0, i+1) using u32 words from the MT.
            let upper = (i + 1) as u32;
            // For small n we can use a single u32; u32 is always enough since n <= usize::MAX
            // but for correctness on large lists use 64-bit rejection.
            let j = if upper == 0 {
                0usize
            } else {
                let threshold = u32::MAX - (u32::MAX % upper);
                let idx_32 = loop {
                    let v = rng.rand_u32();
                    // Accept if v < threshold or threshold wraps (power-of-2 upper)
                    if threshold == 0 || v < threshold {
                        break v % upper;
                    }
                };
                idx_32 as usize
            };
            vec.swap(i, j);
        }

        MoltObject::none().bits()
    })
}

/// Box-Muller Gaussian variate with caching.
/// `mu_bits`, `sigma_bits` — floats or ints.
#[unsafe(no_mangle)]
pub extern "C" fn molt_random_gauss(handle_bits: u64, mu_bits: u64, sigma_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = rng_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let Some(mu) = f64_from_bits(_py, mu_bits, "mu") else {
            return MoltObject::none().bits();
        };
        let Some(sigma) = f64_from_bits(_py, sigma_bits, "sigma") else {
            return MoltObject::none().bits();
        };
        let mut reg = RANDOM_REGISTRY.lock().unwrap();
        let Some(rng) = reg.get_mut(&id) else {
            return raise_exception::<u64>(_py, "ValueError", "invalid Random handle");
        };
        MoltObject::from_float(rng.gauss(mu, sigma)).bits()
    })
}

/// Uniform distribution: a + (b - a) * random().
#[unsafe(no_mangle)]
pub extern "C" fn molt_random_uniform(handle_bits: u64, a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = rng_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let Some(a) = f64_from_bits(_py, a_bits, "a") else {
            return MoltObject::none().bits();
        };
        let Some(b) = f64_from_bits(_py, b_bits, "b") else {
            return MoltObject::none().bits();
        };
        let mut reg = RANDOM_REGISTRY.lock().unwrap();
        let Some(rng) = reg.get_mut(&id) else {
            return raise_exception::<u64>(_py, "ValueError", "invalid Random handle");
        };
        MoltObject::from_float(a + (b - a) * rng.random()).bits()
    })
}

/// Triangular distribution.
/// `low_bits`, `high_bits`, `mode_bits` — float/int or None for mode.
///
/// If mode is None, uses (low + high) / 2.
#[unsafe(no_mangle)]
pub extern "C" fn molt_random_triangular(
    handle_bits: u64,
    low_bits: u64,
    high_bits: u64,
    mode_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = rng_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let Some(low) = f64_from_bits(_py, low_bits, "low") else {
            return MoltObject::none().bits();
        };
        let Some(high) = f64_from_bits(_py, high_bits, "high") else {
            return MoltObject::none().bits();
        };

        let mode_obj = obj_from_bits(mode_bits);
        let c = if mode_obj.is_none() {
            0.5
        } else {
            let Some(m) = f64_from_bits(_py, mode_bits, "mode") else {
                return MoltObject::none().bits();
            };
            if (high - low).abs() < f64::EPSILON {
                0.5
            } else {
                (m - low) / (high - low)
            }
        };

        let mut reg = RANDOM_REGISTRY.lock().unwrap();
        let Some(rng) = reg.get_mut(&id) else {
            return raise_exception::<u64>(_py, "ValueError", "invalid Random handle");
        };
        let u = rng.random();
        let result = if u > c {
            high - (high - low) * (1.0 - c) * (1.0 - u).sqrt()
        } else {
            low + (high - low) * c * u.sqrt()
        };
        MoltObject::from_float(result).bits()
    })
}

/// Exponential distribution: -log(random()) / lambd.
#[unsafe(no_mangle)]
pub extern "C" fn molt_random_expovariate(handle_bits: u64, lambd_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = rng_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let Some(lambd) = f64_from_bits(_py, lambd_bits, "lambd") else {
            return MoltObject::none().bits();
        };
        if lambd == 0.0 {
            return raise_exception::<u64>(_py, "ZeroDivisionError", "lambd must not be zero");
        }
        let mut reg = RANDOM_REGISTRY.lock().unwrap();
        let Some(rng) = reg.get_mut(&id) else {
            return raise_exception::<u64>(_py, "ValueError", "invalid Random handle");
        };
        // CPython: -log(random()) / lambd, guarding against log(0)
        let u = loop {
            let v = rng.random();
            if v > 0.0 {
                break v;
            }
        };
        MoltObject::from_float(-rng_log(u) / lambd).bits()
    })
}

/// Normal variate using Kinderman-Monahan (CPython random.normalvariate).
#[unsafe(no_mangle)]
pub extern "C" fn molt_random_normalvariate(
    handle_bits: u64,
    mu_bits: u64,
    sigma_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = rng_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let Some(mu) = f64_from_bits(_py, mu_bits, "mu") else {
            return MoltObject::none().bits();
        };
        let Some(sigma) = f64_from_bits(_py, sigma_bits, "sigma") else {
            return MoltObject::none().bits();
        };
        let mut reg = RANDOM_REGISTRY.lock().unwrap();
        let Some(rng) = reg.get_mut(&id) else {
            return raise_exception::<u64>(_py, "ValueError", "invalid Random handle");
        };
        MoltObject::from_float(rng.normalvariate(mu, sigma)).bits()
    })
}

/// Log-normal variate: exp(normalvariate(mu, sigma)).
#[unsafe(no_mangle)]
pub extern "C" fn molt_random_lognormvariate(
    handle_bits: u64,
    mu_bits: u64,
    sigma_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = rng_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let Some(mu) = f64_from_bits(_py, mu_bits, "mu") else {
            return MoltObject::none().bits();
        };
        let Some(sigma) = f64_from_bits(_py, sigma_bits, "sigma") else {
            return MoltObject::none().bits();
        };
        let mut reg = RANDOM_REGISTRY.lock().unwrap();
        let Some(rng) = reg.get_mut(&id) else {
            return raise_exception::<u64>(_py, "ValueError", "invalid Random handle");
        };
        let nv = rng.normalvariate(mu, sigma);
        MoltObject::from_float(rng_exp(nv)).bits()
    })
}

/// Von Mises variate (CPython random.vonmisesvariate).
#[unsafe(no_mangle)]
pub extern "C" fn molt_random_vonmisesvariate(
    handle_bits: u64,
    mu_bits: u64,
    kappa_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = rng_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let Some(mu) = f64_from_bits(_py, mu_bits, "mu") else {
            return MoltObject::none().bits();
        };
        let Some(kappa) = f64_from_bits(_py, kappa_bits, "kappa") else {
            return MoltObject::none().bits();
        };

        let mut reg = RANDOM_REGISTRY.lock().unwrap();
        let Some(rng) = reg.get_mut(&id) else {
            return raise_exception::<u64>(_py, "ValueError", "invalid Random handle");
        };

        use core::f64::consts::{PI, TAU};

        let result = if kappa <= 1e-6 {
            // Uniform on [0, 2π)
            TAU * rng.random()
        } else {
            let s = 0.5 / kappa;
            let r = s + rng_sqrt(1.0 + s * s);
            loop {
                let u1 = rng.random();
                let z = rng_cos(PI * u1);
                let d = z / (r + z);
                let u2 = rng.random();
                if u2 < 1.0 - d * d || u2 <= (1.0 - d) * rng_exp(d) {
                    let q = 1.0 / r;
                    let f = (q + z) / (1.0 + q * z);
                    let u3 = rng.random();
                    let theta = if u3 > 0.5 {
                        (mu + rng_atan(f)) % TAU
                    } else {
                        (mu - rng_atan(f)) % TAU
                    };
                    break if theta < 0.0 { theta + TAU } else { theta };
                }
            }
        };

        MoltObject::from_float(result).bits()
    })
}

/// Pareto distribution: 1.0 / pow(random(), 1.0 / alpha).
#[unsafe(no_mangle)]
pub extern "C" fn molt_random_paretovariate(handle_bits: u64, alpha_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = rng_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let Some(alpha) = f64_from_bits(_py, alpha_bits, "alpha") else {
            return MoltObject::none().bits();
        };
        let mut reg = RANDOM_REGISTRY.lock().unwrap();
        let Some(rng) = reg.get_mut(&id) else {
            return raise_exception::<u64>(_py, "ValueError", "invalid Random handle");
        };
        let u = loop {
            let v = rng.random();
            if v > 0.0 {
                break v;
            }
        };
        MoltObject::from_float(u.powf(-1.0 / alpha)).bits()
    })
}

/// Weibull distribution.
#[unsafe(no_mangle)]
pub extern "C" fn molt_random_weibullvariate(
    handle_bits: u64,
    alpha_bits: u64,
    beta_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = rng_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let Some(alpha) = f64_from_bits(_py, alpha_bits, "alpha") else {
            return MoltObject::none().bits();
        };
        let Some(beta) = f64_from_bits(_py, beta_bits, "beta") else {
            return MoltObject::none().bits();
        };
        let mut reg = RANDOM_REGISTRY.lock().unwrap();
        let Some(rng) = reg.get_mut(&id) else {
            return raise_exception::<u64>(_py, "ValueError", "invalid Random handle");
        };
        let u = loop {
            let v = rng.random();
            if v > 0.0 {
                break v;
            }
        };
        MoltObject::from_float(alpha * (-rng_log(u)).powf(1.0 / beta)).bits()
    })
}

/// Gamma variate (CPython random.gammavariate).
/// alpha > 0, beta > 0.
#[unsafe(no_mangle)]
pub extern "C" fn molt_random_gammavariate(
    handle_bits: u64,
    alpha_bits: u64,
    beta_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = rng_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let Some(alpha) = f64_from_bits(_py, alpha_bits, "alpha") else {
            return MoltObject::none().bits();
        };
        let Some(beta) = f64_from_bits(_py, beta_bits, "beta") else {
            return MoltObject::none().bits();
        };

        if alpha <= 0.0 || beta <= 0.0 {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "gammavariate: alpha and beta must be > 0.0",
            );
        }

        let mut reg = RANDOM_REGISTRY.lock().unwrap();
        let Some(rng) = reg.get_mut(&id) else {
            return raise_exception::<u64>(_py, "ValueError", "invalid Random handle");
        };

        // CPython uses two algorithms:
        //   alpha < 1.0 → Ahrens-Dieter
        //   alpha >= 1.0 → Marsaglia-Tsang (GS+GD algorithm)
        let result = if alpha < 1.0 {
            // Ahrens-Dieter (1982) for alpha in (0,1):
            // gammavariate(alpha + 1, 1) * random() ** (1/alpha)
            let alpha1 = alpha + 1.0;
            let x = gamma_marsaglia(rng, alpha1);
            x * rng.random().powf(1.0 / alpha)
        } else {
            gamma_marsaglia(rng, alpha)
        };

        MoltObject::from_float(result * beta).bits()
    })
}

/// Marsaglia-Tsang (2000) fast gamma generator for alpha >= 1.
/// Used internally by gammavariate and betavariate.
fn gamma_marsaglia(rng: &mut MersenneTwisterRng, alpha: f64) -> f64 {
    // Algorithm from G. Marsaglia, W.W. Tsang, "A Simple Method for Generating Gamma Variables"
    // ACM TOMS, Vol. 26, No. 3, 2000, pp. 363-372.
    let d = alpha - 1.0 / 3.0;
    let c = 1.0 / rng_sqrt(9.0 * d);
    loop {
        let x = rng.normalvariate(0.0, 1.0);
        let v_raw = 1.0 + c * x;
        if v_raw <= 0.0 {
            continue;
        }
        let v = v_raw * v_raw * v_raw;
        let u = rng.random();
        let x2 = x * x;
        if u < 1.0 - 0.0331 * (x2 * x2) {
            return d * v;
        }
        if rng_log(u) < 0.5 * x2 + d * (1.0 - v + rng_log(v)) {
            return d * v;
        }
    }
}

/// Beta variate using two gamma variates (CPython random.betavariate).
/// alpha > 0, beta > 0.
#[unsafe(no_mangle)]
pub extern "C" fn molt_random_betavariate(
    handle_bits: u64,
    alpha_bits: u64,
    beta_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = rng_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };
        let Some(alpha) = f64_from_bits(_py, alpha_bits, "alpha") else {
            return MoltObject::none().bits();
        };
        let Some(beta) = f64_from_bits(_py, beta_bits, "beta") else {
            return MoltObject::none().bits();
        };

        if alpha <= 0.0 || beta <= 0.0 {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "betavariate: alpha and beta must be > 0.0",
            );
        }

        let mut reg = RANDOM_REGISTRY.lock().unwrap();
        let Some(rng) = reg.get_mut(&id) else {
            return raise_exception::<u64>(_py, "ValueError", "invalid Random handle");
        };

        let y = if alpha < 1.0 {
            gamma_marsaglia(rng, alpha + 1.0) * rng.random().powf(1.0 / alpha)
        } else {
            gamma_marsaglia(rng, alpha)
        };
        let z = if beta < 1.0 {
            gamma_marsaglia(rng, beta + 1.0) * rng.random().powf(1.0 / beta)
        } else {
            gamma_marsaglia(rng, beta)
        };

        let denom = y + z;
        let result = if denom == 0.0 { 0.0 } else { y / denom };
        MoltObject::from_float(result).bits()
    })
}

/// Weighted random choices (with replacement).
///
/// `population_bits` — list or tuple.
/// `cum_weights_bits` — list/tuple of cumulative floats, or None (uniform).
/// `k_bits` — integer count.
///
/// Returns a new list of k elements.
#[unsafe(no_mangle)]
pub extern "C" fn molt_random_choices(
    handle_bits: u64,
    population_bits: u64,
    cum_weights_bits: u64,
    k_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = rng_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };

        let pop_obj = obj_from_bits(population_bits);
        let Some(pop_ptr) = pop_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "population must be a sequence");
        };
        let pop_type = unsafe { object_type_id(pop_ptr) };
        if pop_type != TYPE_ID_LIST && pop_type != TYPE_ID_TUPLE {
            return raise_exception::<u64>(_py, "TypeError", "population must be a list or tuple");
        }
        let pop_elems = unsafe { seq_vec_ref(pop_ptr) };
        let pop_len = pop_elems.len();

        if pop_len == 0 {
            return raise_exception::<u64>(
                _py,
                "IndexError",
                "cannot choose from an empty sequence",
            );
        }

        let k_obj = obj_from_bits(k_bits);
        let Some(k_i64) = to_i64(k_obj) else {
            return raise_exception::<u64>(_py, "TypeError", "k must be an integer");
        };
        if k_i64 < 0 {
            return raise_exception::<u64>(_py, "ValueError", "k must be non-negative");
        }
        let k = k_i64 as usize;

        // Build cumulative weights as f64 vec (or uniform)
        let cum_weights_obj = obj_from_bits(cum_weights_bits);
        let cum_weights: Option<Vec<f64>> = if cum_weights_obj.is_none() {
            None
        } else {
            let Some(cw_ptr) = cum_weights_obj.as_ptr() else {
                return raise_exception::<u64>(_py, "TypeError", "cum_weights must be a sequence");
            };
            let cw_type = unsafe { object_type_id(cw_ptr) };
            if cw_type != TYPE_ID_LIST && cw_type != TYPE_ID_TUPLE {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "cum_weights must be a list or tuple",
                );
            }
            let cw_elems = unsafe { seq_vec_ref(cw_ptr) };
            if cw_elems.len() != pop_len {
                return raise_exception::<u64>(
                    _py,
                    "ValueError",
                    "cum_weights must have the same length as population",
                );
            }
            let mut weights = Vec::with_capacity(pop_len);
            for &w_bits in cw_elems {
                let Some(w) = f64_from_bits(_py, w_bits, "cum_weight") else {
                    return MoltObject::none().bits();
                };
                weights.push(w);
            }
            Some(weights)
        };

        let mut reg = RANDOM_REGISTRY.lock().unwrap();
        let Some(rng) = reg.get_mut(&id) else {
            return raise_exception::<u64>(_py, "ValueError", "invalid Random handle");
        };

        let mut result_bits: Vec<u64> = Vec::with_capacity(k);
        for _ in 0..k {
            let idx = match &cum_weights {
                None => {
                    // Uniform: randbelow(pop_len) using rejection sampling
                    let n = pop_len as u32;
                    let threshold = u32::MAX - (u32::MAX % n);
                    loop {
                        let v = rng.rand_u32();
                        if threshold == 0 || v < threshold {
                            break (v % n) as usize;
                        }
                    }
                }
                Some(cw) => {
                    let total = cw[cw.len() - 1];
                    let u = rng.random() * total;
                    // Binary search for first cw[i] >= u
                    cw.partition_point(|&w| w < u).min(pop_len - 1)
                }
            };
            result_bits.push(pop_elems[idx]);
        }

        // alloc_list inc_refs each element internally, so we pass result_bits directly.
        match alloc_list_bits(_py, &result_bits) {
            Ok(list_bits) => list_bits,
            Err(exc) => exc,
        }
    })
}

/// Random sample without replacement.
///
/// `population_bits` — list or tuple.
/// `k_bits` — integer count.
///
/// Returns a new list of k distinct elements (by position).
#[unsafe(no_mangle)]
pub extern "C" fn molt_random_sample(handle_bits: u64, population_bits: u64, k_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = rng_handle_from_bits(_py, handle_bits) else {
            return MoltObject::none().bits();
        };

        let pop_obj = obj_from_bits(population_bits);
        let Some(pop_ptr) = pop_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "population must be a sequence");
        };
        let pop_type = unsafe { object_type_id(pop_ptr) };
        if pop_type != TYPE_ID_LIST && pop_type != TYPE_ID_TUPLE {
            return raise_exception::<u64>(_py, "TypeError", "population must be a list or tuple");
        }
        let pop_elems = unsafe { seq_vec_ref(pop_ptr) };
        let n = pop_elems.len();

        let k_obj = obj_from_bits(k_bits);
        let Some(k_i64) = to_i64(k_obj) else {
            return raise_exception::<u64>(_py, "TypeError", "k must be an integer");
        };
        if k_i64 < 0 {
            return raise_exception::<u64>(_py, "ValueError", "sample k must be non-negative");
        }
        let k = k_i64 as usize;
        if k > n {
            return raise_exception::<u64>(_py, "ValueError", "sample k larger than population");
        }

        let mut reg = RANDOM_REGISTRY.lock().unwrap();
        let Some(rng) = reg.get_mut(&id) else {
            return raise_exception::<u64>(_py, "ValueError", "invalid Random handle");
        };

        // For small k relative to n, use a set-based approach.
        // For larger k, copy + partial Fisher-Yates.
        let result_bits: Vec<u64> = if k * 3 <= n * 2 {
            // Small sample: track selected indices in a HashSet
            let mut selected = std::collections::HashSet::with_capacity(k);
            let mut result = Vec::with_capacity(k);
            while result.len() < k {
                let upper = n as u32;
                let threshold = u32::MAX - (u32::MAX % upper);
                let idx = loop {
                    let v = rng.rand_u32();
                    if threshold == 0 || v < threshold {
                        break (v % upper) as usize;
                    }
                };
                if selected.insert(idx) {
                    result.push(pop_elems[idx]);
                }
            }
            result
        } else {
            // Large sample: partial Fisher-Yates on a copy
            let mut indices: Vec<usize> = (0..n).collect();
            for i in 0..k {
                let remaining = (n - i) as u32;
                let threshold = u32::MAX - (u32::MAX % remaining);
                let j = i + loop {
                    let v = rng.rand_u32();
                    if threshold == 0 || v < threshold {
                        break (v % remaining) as usize;
                    }
                };
                indices.swap(i, j);
            }
            indices[..k].iter().map(|&idx| pop_elems[idx]).collect()
        };

        // alloc_list inc_refs each element internally, so we pass result_bits directly.
        match alloc_list_bits(_py, &result_bits) {
            Ok(list_bits) => list_bits,
            Err(exc) => exc,
        }
    })
}

// ─── Additional math helpers for binomialvariate ────────────────────────────

#[inline(always)]
fn rng_lgamma(x: f64) -> f64 {
    libm::lgamma(x)
}

// ─── binomialvariate ────────────────────────────────────────────────────────

/// Mirrors CPython random.Random.binomialvariate exactly.
/// Parameters: handle, n (int), p (float).
#[unsafe(no_mangle)]
pub extern "C" fn molt_random_binomialvariate(handle_bits: u64, n_bits: u64, p_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = rng_handle_from_bits(_py, handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "Random handle must be an int");
        };
        let Some(n) = to_i64(obj_from_bits(n_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "n must be an integer");
        };
        let Some(p) = f64_from_bits(_py, p_bits, "p") else {
            return raise_exception::<u64>(_py, "TypeError", "p must be a number");
        };

        if n < 0 {
            return raise_exception::<u64>(_py, "ValueError", "n must be non-negative");
        }
        if p <= 0.0 || p >= 1.0 {
            if p == 0.0 {
                return MoltObject::from_int(0).bits();
            }
            if p == 1.0 {
                return MoltObject::from_int(n).bits();
            }
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "p must be in the range 0.0 <= p <= 1.0",
            );
        }

        let mut reg = RANDOM_REGISTRY.lock().unwrap();
        let Some(rng) = reg.get_mut(&id) else {
            return raise_exception::<u64>(_py, "ValueError", "invalid Random handle");
        };

        let result = binomialvariate_impl(rng, n, p);
        MoltObject::from_int(result).bits()
    })
}

fn binomialvariate_impl(rng: &mut MersenneTwisterRng, n: i64, p: f64) -> i64 {
    if n == 1 {
        return if rng.random() < p { 1 } else { 0 };
    }

    if p > 0.5 {
        return n - binomialvariate_impl(rng, n, 1.0 - p);
    }

    let nf = n as f64;
    if nf * p < 10.0 {
        // BTPE inverse transform
        let mut x: i64 = 0;
        let mut y: i64 = 0;
        let c = rng_log2(1.0 - p);
        if c == 0.0 {
            return x;
        }
        loop {
            y += rng_floor(rng_log2(rng.random()) / c) as i64 + 1;
            if y > n {
                return x;
            }
            x += 1;
        }
    }

    // BTPE algorithm
    let spq = rng_sqrt(nf * p * (1.0 - p));
    let b = 1.15 + 2.53 * spq;
    let a = -0.0873 + 0.0248 * b + 0.01 * p;
    let c_val = nf * p + 0.5;
    let vr = 0.92 - 4.2 / b;

    let mut setup_complete = false;
    let mut alpha = 0.0_f64;
    let mut lpq = 0.0_f64;
    let mut m = 0.0_f64;
    let mut h = 0.0_f64;

    loop {
        let u = rng.random();
        let u2 = u - 0.5;
        let us = 0.5 - rng_fabs(u2);
        let k = rng_floor((2.0 * a / us + b) * u2 + c_val) as i64;
        if k < 0 || k > n {
            continue;
        }

        let v = rng.random();
        if us >= 0.07 && v <= vr {
            return k;
        }

        if !setup_complete {
            alpha = (2.83 + 5.1 / b) * spq;
            lpq = rng_log(p / (1.0 - p));
            m = rng_floor((nf + 1.0) * p);
            h = rng_lgamma(m + 1.0) + rng_lgamma(nf - m + 1.0);
            setup_complete = true;
        }
        let kf = k as f64;
        let v2 = v * alpha / (a / (us * us) + b);
        if rng_log(v2) <= h - rng_lgamma(kf + 1.0) - rng_lgamma(nf - kf + 1.0) + (kf - m) * lpq {
            return k;
        }
    }
}

/// randrange(start, stop, step) -> int
/// All validation and range arithmetic done in Rust.
#[unsafe(no_mangle)]
pub extern "C" fn molt_random_randrange(
    handle_bits: u64,
    start_bits: u64,
    stop_bits: u64,
    step_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = rng_handle_from_bits(_py, handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "Random handle must be an int");
        };
        let Some(istart) = to_i64(obj_from_bits(start_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "integer argument expected");
        };
        // stop = None is encoded as MoltObject::none()
        let stop_obj = obj_from_bits(stop_bits);
        let istop_opt = if stop_obj.is_none() {
            None
        } else {
            to_i64(stop_obj)
        };
        let Some(istep) = to_i64(obj_from_bits(step_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "integer argument expected");
        };

        let mut reg = RANDOM_REGISTRY.lock().unwrap();
        let Some(rng) = reg.get_mut(&id) else {
            return raise_exception::<u64>(_py, "ValueError", "invalid Random handle");
        };

        if istop_opt.is_none() {
            if istep != 1 {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "Missing a non-None stop argument",
                );
            }
            if istart <= 0 {
                return raise_exception::<u64>(_py, "ValueError", "empty range for randrange()");
            }
            let n_big = BigInt::from(istart);
            let result = rng.randbelow(&n_big);
            return int_bits_from_bigint(_py, result);
        }

        let istop = istop_opt.unwrap();
        let width = istop - istart;
        if istep == 1 {
            if width <= 0 {
                return raise_exception::<u64>(_py, "ValueError", "empty range for randrange()");
            }
            let n_big = BigInt::from(width);
            let r = rng.randbelow(&n_big);
            let result = BigInt::from(istart) + r;
            return int_bits_from_bigint(_py, result);
        }

        if istep == 0 {
            return raise_exception::<u64>(_py, "ValueError", "zero step for randrange()");
        }

        let n = if istep > 0 {
            (width + istep - 1) / istep
        } else {
            (width + istep + 1) / istep
        };
        if n <= 0 {
            return raise_exception::<u64>(_py, "ValueError", "empty range for randrange()");
        }

        let n_big = BigInt::from(n);
        let r = rng.randbelow(&n_big);
        let result = BigInt::from(istart) + BigInt::from(istep) * r;
        int_bits_from_bigint(_py, result)
    })
}

/// randbytes(n) -> generates n random bytes
#[unsafe(no_mangle)]
pub extern "C" fn molt_random_randbytes(handle_bits: u64, n_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(id) = rng_handle_from_bits(_py, handle_bits) else {
            return raise_exception::<u64>(_py, "TypeError", "Random handle must be an int");
        };
        let Some(n) = to_i64(obj_from_bits(n_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "n must be an integer");
        };
        if n < 0 {
            return raise_exception::<u64>(_py, "ValueError", "negative argument not allowed");
        }
        let n = n as usize;

        let mut reg = RANDOM_REGISTRY.lock().unwrap();
        let Some(rng) = reg.get_mut(&id) else {
            return raise_exception::<u64>(_py, "ValueError", "invalid Random handle");
        };

        let total_bits = (n * 8) as u32;
        let big = rng.randbits_biguint(total_bits);
        let bytes_vec = big.to_bytes_le();
        // Pad or truncate to exactly n bytes
        let mut result = vec![0u8; n];
        let copy_len = bytes_vec.len().min(n);
        result[..copy_len].copy_from_slice(&bytes_vec[..copy_len]);

        let ptr = crate::alloc_bytes(_py, &result);
        if ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}
