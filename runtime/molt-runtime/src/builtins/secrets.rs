// === FILE: runtime/molt-runtime/src/builtins/secrets.rs ===
//
// secrets module intrinsics: cryptographically-secure random tokens.
// Uses getrandom::fill (available on all targets including WASM via wasm_js feature).

use getrandom::fill as getrandom_fill;

use crate::object::ops::string_obj_to_owned;
use crate::{
    MoltObject, PyToken, alloc_bytes, alloc_string, int_bits_from_bigint, int_bits_from_i64,
    obj_from_bits, raise_exception, to_i64,
};
use num_bigint::{BigInt, Sign};

// ---------------------------------------------------------------------------
// URL-safe base64 alphabet (RFC 4648 §5, no padding)
// ---------------------------------------------------------------------------

const BASE64URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn base64url_encode(bytes: &[u8]) -> String {
    let mut out = String::with_capacity((bytes.len() * 4).div_ceil(3));
    let mut i = 0usize;
    while i + 2 < bytes.len() {
        let b0 = bytes[i] as u32;
        let b1 = bytes[i + 1] as u32;
        let b2 = bytes[i + 2] as u32;
        out.push(BASE64URL[((b0 >> 2) & 0x3F) as usize] as char);
        out.push(BASE64URL[(((b0 << 4) | (b1 >> 4)) & 0x3F) as usize] as char);
        out.push(BASE64URL[(((b1 << 2) | (b2 >> 6)) & 0x3F) as usize] as char);
        out.push(BASE64URL[(b2 & 0x3F) as usize] as char);
        i += 3;
    }
    let rem = bytes.len() - i;
    if rem == 1 {
        let b0 = bytes[i] as u32;
        out.push(BASE64URL[((b0 >> 2) & 0x3F) as usize] as char);
        out.push(BASE64URL[((b0 << 4) & 0x3F) as usize] as char);
    } else if rem == 2 {
        let b0 = bytes[i] as u32;
        let b1 = bytes[i + 1] as u32;
        out.push(BASE64URL[((b0 >> 2) & 0x3F) as usize] as char);
        out.push(BASE64URL[(((b0 << 4) | (b1 >> 4)) & 0x3F) as usize] as char);
        out.push(BASE64URL[((b1 << 2) & 0x3F) as usize] as char);
    }
    out
}

// ---------------------------------------------------------------------------
// Internal: fill buffer with cryptographic random bytes
// ---------------------------------------------------------------------------

fn fill_random(_py: &PyToken<'_>, buf: &mut [u8]) -> Result<(), u64> {
    getrandom_fill(buf).map_err(|_| raise_exception::<u64>(_py, "OSError", "getrandom failed"))
}

fn resolve_nbytes(_py: &PyToken<'_>, nbytes_bits: u64, default: usize) -> Result<usize, u64> {
    let obj = obj_from_bits(nbytes_bits);
    if obj.is_none() {
        return Ok(default);
    }
    let Some(v) = to_i64(obj) else {
        return Err(raise_exception::<u64>(
            _py,
            "TypeError",
            "nbytes must be an integer",
        ));
    };
    if v < 0 {
        return Err(raise_exception::<u64>(
            _py,
            "ValueError",
            "nbytes must be non-negative",
        ));
    }
    Ok(v as usize)
}

// ---------------------------------------------------------------------------
// Public intrinsics
// ---------------------------------------------------------------------------

#[unsafe(no_mangle)]
pub extern "C" fn molt_secrets_token_bytes(nbytes_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let n = match resolve_nbytes(_py, nbytes_bits, 32) {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let mut buf = vec![0u8; n];
        if let Err(exc) = fill_random(_py, &mut buf) {
            return exc;
        }
        let ptr = alloc_bytes(_py, &buf);
        if ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_secrets_token_hex(nbytes_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let n = match resolve_nbytes(_py, nbytes_bits, 32) {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let mut buf = vec![0u8; n];
        if let Err(exc) = fill_random(_py, &mut buf) {
            return exc;
        }
        let hex: String = buf.iter().map(|b| format!("{b:02x}")).collect();
        let ptr = alloc_string(_py, hex.as_bytes());
        if ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_secrets_token_urlsafe(nbytes_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let n = match resolve_nbytes(_py, nbytes_bits, 32) {
            Ok(v) => v,
            Err(exc) => return exc,
        };
        let mut buf = vec![0u8; n];
        if let Err(exc) = fill_random(_py, &mut buf) {
            return exc;
        }
        let encoded = base64url_encode(&buf);
        let ptr = alloc_string(_py, encoded.as_bytes());
        if ptr.is_null() {
            return raise_exception::<u64>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_secrets_randbits(k_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(k) = to_i64(obj_from_bits(k_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "k must be an integer");
        };
        if k < 0 {
            return raise_exception::<u64>(
                _py,
                "ValueError",
                "number of bits must be non-negative",
            );
        }
        if k == 0 {
            return int_bits_from_i64(_py, 0);
        }
        let nbytes = (k as usize).div_ceil(8);
        let mut buf = vec![0u8; nbytes];
        if let Err(exc) = fill_random(_py, &mut buf) {
            return exc;
        }
        // Mask off excess bits in the top byte.
        let excess = nbytes * 8 - k as usize;
        if excess > 0 {
            buf[0] &= 0xFF >> excess;
        }
        let big = BigInt::from_bytes_be(Sign::Plus, &buf);
        int_bits_from_bigint(_py, big)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_secrets_compare_digest(a_bits: u64, b_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        // Accept str or bytes-like for both arguments.
        let a_obj = obj_from_bits(a_bits);
        let b_obj = obj_from_bits(b_bits);

        let a_is_str = a_obj
            .as_ptr()
            .is_some_and(|ptr| unsafe { crate::object_type_id(ptr) == crate::TYPE_ID_STRING });
        let b_is_str = b_obj
            .as_ptr()
            .is_some_and(|ptr| unsafe { crate::object_type_id(ptr) == crate::TYPE_ID_STRING });

        if a_is_str && b_is_str {
            let Some(a_text) = string_obj_to_owned(a_obj) else {
                return raise_exception::<u64>(_py, "TypeError", "expected str");
            };
            let Some(b_text) = string_obj_to_owned(b_obj) else {
                return raise_exception::<u64>(_py, "TypeError", "expected str");
            };
            let a_bytes = a_text.as_bytes();
            let b_bytes = b_text.as_bytes();
            if a_bytes.len() != b_bytes.len() {
                return MoltObject::from_bool(false).bits();
            }
            let mut acc: u8 = 0;
            for (l, r) in a_bytes.iter().zip(b_bytes.iter()) {
                acc |= l ^ r;
            }
            return MoltObject::from_bool(acc == 0).bits();
        }

        if a_is_str || b_is_str {
            return raise_exception::<u64>(
                _py,
                "TypeError",
                "both operands must be of the same type (str or bytes-like)",
            );
        }

        // bytes-like path
        let a_slice = a_obj
            .as_ptr()
            .and_then(|ptr| unsafe { crate::bytes_like_slice(ptr) });
        let b_slice = b_obj
            .as_ptr()
            .and_then(|ptr| unsafe { crate::bytes_like_slice(ptr) });

        match (a_slice, b_slice) {
            (Some(a), Some(b)) => {
                if a.len() != b.len() {
                    return MoltObject::from_bool(false).bits();
                }
                let mut acc: u8 = 0;
                for (l, r) in a.iter().zip(b.iter()) {
                    acc |= l ^ r;
                }
                MoltObject::from_bool(acc == 0).bits()
            }
            _ => raise_exception::<u64>(_py, "TypeError", "a bytes-like object is required"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_secrets_choice(seq_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let seq_obj = obj_from_bits(seq_bits);
        let Some(seq_ptr) = seq_obj.as_ptr() else {
            return raise_exception::<u64>(_py, "TypeError", "sequence must not be None");
        };
        let len = unsafe {
            let type_id = crate::object_type_id(seq_ptr);
            if type_id == crate::TYPE_ID_LIST || type_id == crate::TYPE_ID_TUPLE {
                crate::list_len(seq_ptr)
            } else {
                return raise_exception::<u64>(
                    _py,
                    "TypeError",
                    "secrets.choice requires a sequence",
                );
            }
        };
        if len == 0 {
            return raise_exception::<u64>(
                _py,
                "IndexError",
                "cannot choose from an empty sequence",
            );
        }
        // Generate a uniformly random index via rejection sampling.
        let idx = match random_index_below(_py, len) {
            Ok(i) => i,
            Err(exc) => return exc,
        };
        let seq_vec_ptr = unsafe { crate::seq_vec_ptr(seq_ptr) };
        let seq = unsafe { &*seq_vec_ptr };
        let elem_bits = seq[idx];
        crate::inc_ref_bits(_py, elem_bits);
        elem_bits
    })
}

// Returns a random index in [0, upper) via rejection sampling, or an exception bits.
fn random_index_below(_py: &PyToken<'_>, upper: usize) -> Result<usize, u64> {
    if upper == 0 {
        return Err(raise_exception::<u64>(
            _py,
            "ValueError",
            "upper must be > 0",
        ));
    }
    // Use 8 random bytes (u64) and rejection-sample to avoid modulo bias.
    let mut buf = [0u8; 8];
    let threshold = u64::MAX - (u64::MAX % upper as u64);
    loop {
        getrandom_fill(&mut buf)
            .map_err(|_| raise_exception::<u64>(_py, "OSError", "getrandom failed"))?;
        let v = u64::from_le_bytes(buf);
        if v < threshold || threshold == 0 {
            return Ok((v % upper as u64) as usize);
        }
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_secrets_below(upper_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(upper_i64) = to_i64(obj_from_bits(upper_bits)) else {
            return raise_exception::<u64>(_py, "TypeError", "upper must be an integer");
        };
        if upper_i64 <= 0 {
            return raise_exception::<u64>(_py, "ValueError", "upper must be positive");
        }
        let upper = upper_i64 as usize;
        match random_index_below(_py, upper) {
            Ok(idx) => int_bits_from_i64(_py, idx as i64),
            Err(exc) => exc,
        }
    })
}
