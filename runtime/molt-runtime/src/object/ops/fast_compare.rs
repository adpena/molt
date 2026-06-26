use super::*;

/// Fast path: compare two NaN-boxed integers (COMPARE_OP_INT).
///
/// `op` encodes the comparison:
///   0 = Lt, 1 = Le, 2 = Eq, 3 = Ne, 4 = Gt, 5 = Ge
///
/// If either operand is not a NaN-boxed int the call falls through to the
/// appropriate generic comparison function.  Both booleans and int subclasses
/// are handled by the slow path.
#[unsafe(no_mangle)]
pub extern "C" fn molt_compare_int_fast(a: u64, b: u64, op: u32) -> u64 {
    let lhs = obj_from_bits(a);
    let rhs = obj_from_bits(b);
    // Both operands must be plain NaN-boxed ints (not bools, not subclasses).
    if lhs.is_int() && rhs.is_int() {
        let li = lhs.as_int_unchecked();
        let ri = rhs.as_int_unchecked();
        let result = match op {
            0 => li < ri,
            1 => li <= ri,
            2 => li == ri,
            3 => li != ri,
            4 => li > ri,
            5 => li >= ri,
            _ => return molt_eq(a, b), // unknown op: safe fallback
        };
        return MoltObject::from_bool(result).bits();
    }
    // Slow path: delegate to the full generic comparison.
    match op {
        0 => molt_lt(a, b),
        1 => molt_le(a, b),
        2 => molt_eq(a, b),
        3 => molt_ne(a, b),
        4 => molt_gt(a, b),
        5 => molt_ge(a, b),
        _ => molt_eq(a, b),
    }
}

/// Fast path: string equality using pointer identity (COMPARE_OP_STR).
///
/// In Molt, every string object has a unique allocation, so pointer equality
/// immediately proves equality.  If the pointers are the same we return `true`
/// without touching the bytes.  If they differ we fall back to the byte-wise
/// comparison already inside `molt_string_eq`.
///
/// This function is intentionally `unsafe`-free at the call site — it wraps
/// the unsafe pointer dereferences internally and returns an `Option`:
///   `Some(true)`  — pointers equal → strings equal
///   `Some(false)` — strings are TYPE_ID_STRING but different lengths
///   `None`        — one or both operands are not strings; caller should
///                   fall through to `molt_eq`
#[inline]
fn string_eq_fast(a: u64, b: u64) -> Option<bool> {
    let lhs = obj_from_bits(a);
    let rhs = obj_from_bits(b);
    let lp = lhs.as_ptr()?;
    let rp = rhs.as_ptr()?;
    unsafe {
        if object_type_id(lp) != TYPE_ID_STRING || object_type_id(rp) != TYPE_ID_STRING {
            return None;
        }
        // Pointer equality: same allocation → same content.
        if lp == rp {
            return Some(true);
        }
        // Length mismatch: definitely not equal (avoids byte scan).
        let l_len = string_len(lp);
        let r_len = string_len(rp);
        if l_len != r_len {
            return Some(false);
        }
        // Fall through to byte comparison.
        None
    }
}

/// Extern fast-path wrapper for string equality (COMPARE_OP_STR).
///
/// Uses `string_eq_fast` for the pointer/length checks, then delegates to
/// `molt_string_eq` for byte comparison when needed.
#[unsafe(no_mangle)]
pub extern "C" fn molt_string_eq_fast(a: u64, b: u64) -> u64 {
    match string_eq_fast(a, b) {
        Some(result) => MoltObject::from_bool(result).bits(),
        None => molt_string_eq(a, b),
    }
}
