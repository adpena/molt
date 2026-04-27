// Comparison operations and helpers.
// Split from ops.rs for compilation-unit size reduction.

use crate::*;
use molt_obj_model::MoltObject;
use std::cmp::Ordering;

use super::ops::{
    BinaryDunderOutcome, call_dunder_raw, is_float_extended, simd_bytes_eq,
    simd_find_first_mismatch,
};

pub(crate) fn compare_type_error(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
    op: &str,
) -> u64 {
    let msg = format!(
        "'{}' not supported between instances of '{}' and '{}'",
        op,
        type_name(_py, lhs),
        type_name(_py, rhs),
    );
    raise_exception::<_>(_py, "TypeError", &msg)
}

#[derive(Clone, Copy)]
pub(crate) enum CompareOutcome {
    Ordered(Ordering),
    Unordered,
    NotComparable,
    Error,
}

#[derive(Clone, Copy)]
pub(crate) enum CompareBoolOutcome {
    True,
    False,
    NotComparable,
    Error,
}

#[derive(Clone, Copy)]
enum CompareValueOutcome {
    Value(u64),
    NotComparable,
    Error,
}

#[derive(Clone, Copy)]
pub(crate) enum CompareOp {
    Lt,
    Le,
    Gt,
    Ge,
}

fn is_number(obj: MoltObject) -> bool {
    to_i64(obj).is_some() || is_float_extended(obj) || bigint_ptr_from_bits(obj.bits()).is_some()
}

fn compare_numbers_outcome(lhs: MoltObject, rhs: MoltObject) -> CompareOutcome {
    if let Some(ordering) = compare_numbers(lhs, rhs) {
        return CompareOutcome::Ordered(ordering);
    }
    if is_number(lhs) && is_number(rhs) {
        return CompareOutcome::Unordered;
    }
    CompareOutcome::NotComparable
}

// ---------------------------------------------------------------------------
// SIMD-accelerated lexicographic byte comparison for string/bytes ordering.
// Uses SIMD to skip past equal prefix, then scalar compare at divergence.
// ---------------------------------------------------------------------------

/// Find the first byte index where `a` and `b` differ, within `len` bytes.
/// Returns `len` if the prefixes are identical.
#[inline]
unsafe fn simd_find_first_byte_diff(a: *const u8, b: *const u8, len: usize) -> usize {
    unsafe {
        #[cfg(target_arch = "x86_64")]
        {
            if std::arch::is_x86_feature_detected!("avx2") {
                return simd_find_first_byte_diff_avx2(a, b, len);
            }
            return simd_find_first_byte_diff_sse2(a, b, len);
        }
        #[cfg(target_arch = "aarch64")]
        {
            return simd_find_first_byte_diff_neon(a, b, len);
        }
        #[cfg(target_arch = "wasm32")]
        {
            if cfg!(target_feature = "simd128") {
                return simd_find_first_byte_diff_wasm(a, b, len);
            }
        }
        #[allow(unreachable_code)]
        {
            for i in 0..len {
                if *a.add(i) != *b.add(i) {
                    return i;
                }
            }
            len
        }
    }
}

#[cfg(target_arch = "wasm32")]
#[inline]
unsafe fn simd_find_first_byte_diff_wasm(a: *const u8, b: *const u8, len: usize) -> usize {
    use std::arch::wasm32::*;
    let mut i = 0usize;
    while i + 16 <= len {
        let va = unsafe { v128_load(a.add(i) as *const v128) };
        let vb = unsafe { v128_load(b.add(i) as *const v128) };
        let eq = u8x16_eq(va, vb);
        let mask = u8x16_bitmask(eq) as u32;
        if mask != 0xFFFF {
            // Not all equal — find first differing byte
            return i + (!mask).trailing_zeros() as usize;
        }
        i += 16;
    }
    // Scalar tail
    while i < len {
        if unsafe { *a.add(i) != *b.add(i) } {
            return i;
        }
        i += 1;
    }
    len
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn simd_find_first_byte_diff_sse2(a: *const u8, b: *const u8, len: usize) -> usize {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    while i + 16 <= len {
        let va = _mm_loadu_si128(a.add(i) as *const __m128i);
        let vb = _mm_loadu_si128(b.add(i) as *const __m128i);
        let cmp = _mm_cmpeq_epi8(va, vb);
        let mask = _mm_movemask_epi8(cmp) as u32;
        if mask != 0xFFFF {
            // Find first differing byte via trailing zeros of negated mask
            return i + (!mask).trailing_zeros() as usize;
        }
        i += 16;
    }
    for j in i..len {
        if *a.add(j) != *b.add(j) {
            return j;
        }
    }
    len
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn simd_find_first_byte_diff_avx2(a: *const u8, b: *const u8, len: usize) -> usize {
    use std::arch::x86_64::*;
    let mut i = 0usize;
    while i + 32 <= len {
        let va = _mm256_loadu_si256(a.add(i) as *const __m256i);
        let vb = _mm256_loadu_si256(b.add(i) as *const __m256i);
        let cmp = _mm256_cmpeq_epi8(va, vb);
        let mask = _mm256_movemask_epi8(cmp) as u32;
        if mask != 0xFFFFFFFF {
            return i + (!mask).trailing_zeros() as usize;
        }
        i += 32;
    }
    // SSE2 tail
    if i + 16 <= len {
        let va = _mm_loadu_si128(a.add(i) as *const __m128i);
        let vb = _mm_loadu_si128(b.add(i) as *const __m128i);
        let cmp = _mm_cmpeq_epi8(va, vb);
        let mask = _mm_movemask_epi8(cmp) as u32;
        if mask != 0xFFFF {
            return i + (!mask).trailing_zeros() as usize;
        }
        i += 16;
    }
    for j in i..len {
        if *a.add(j) != *b.add(j) {
            return j;
        }
    }
    len
}

#[cfg(target_arch = "aarch64")]
#[inline]
unsafe fn simd_find_first_byte_diff_neon(a: *const u8, b: *const u8, len: usize) -> usize {
    unsafe {
        use std::arch::aarch64::*;
        let mut i = 0usize;
        while i + 16 <= len {
            let va = vld1q_u8(a.add(i));
            let vb = vld1q_u8(b.add(i));
            let cmp = vceqq_u8(va, vb);
            if vminvq_u8(cmp) != 0xFF {
                // Find the exact byte — check 8-byte halves first
                let low = vget_low_u8(cmp);
                let _high = vget_high_u8(cmp);
                if vminv_u8(low) != 0xFF {
                    for j in 0..8 {
                        if *a.add(i + j) != *b.add(i + j) {
                            return i + j;
                        }
                    }
                }
                for j in 8..16 {
                    if *a.add(i + j) != *b.add(i + j) {
                        return i + j;
                    }
                }
            }
            i += 16;
        }
        for j in i..len {
            if *a.add(j) != *b.add(j) {
                return j;
            }
        }
        len
    }
}

unsafe fn compare_string_bytes(lhs_ptr: *mut u8, rhs_ptr: *mut u8) -> Ordering {
    unsafe {
        let l_len = string_len(lhs_ptr);
        let r_len = string_len(rhs_ptr);
        let common = l_len.min(r_len);
        if common >= 32 {
            // SIMD fast path: skip past identical prefix
            let l_data = string_bytes(lhs_ptr);
            let r_data = string_bytes(rhs_ptr);
            let diff_at = simd_find_first_byte_diff(l_data, r_data, common);
            if diff_at == common {
                return l_len.cmp(&r_len);
            }
            return (*l_data.add(diff_at)).cmp(&*r_data.add(diff_at));
        }
        let l_bytes = std::slice::from_raw_parts(string_bytes(lhs_ptr), l_len);
        let r_bytes = std::slice::from_raw_parts(string_bytes(rhs_ptr), r_len);
        l_bytes.cmp(r_bytes)
    }
}

unsafe fn compare_bytes_like(lhs_ptr: *mut u8, rhs_ptr: *mut u8) -> Ordering {
    unsafe {
        let l_len = bytes_len(lhs_ptr);
        let r_len = bytes_len(rhs_ptr);
        let common = l_len.min(r_len);
        if common >= 32 {
            // SIMD fast path: skip past identical prefix
            let l_data = bytes_data(lhs_ptr);
            let r_data = bytes_data(rhs_ptr);
            let diff_at = simd_find_first_byte_diff(l_data, r_data, common);
            if diff_at == common {
                return l_len.cmp(&r_len);
            }
            return (*l_data.add(diff_at)).cmp(&*r_data.add(diff_at));
        }
        let l_bytes = std::slice::from_raw_parts(bytes_data(lhs_ptr), l_len);
        let r_bytes = std::slice::from_raw_parts(bytes_data(rhs_ptr), r_len);
        l_bytes.cmp(r_bytes)
    }
}

unsafe fn compare_sequence(
    _py: &PyToken<'_>,
    lhs_ptr: *mut u8,
    rhs_ptr: *mut u8,
) -> CompareOutcome {
    unsafe {
        let lhs = seq_vec_ref(lhs_ptr);
        let rhs = seq_vec_ref(rhs_ptr);
        let common = lhs.len().min(rhs.len());
        // SIMD fast path: bulk-compare NaN-boxed u64 arrays to skip past
        // identity-equal prefix without per-element branch overhead.
        let first_diff = simd_find_first_mismatch(lhs, rhs);
        for idx in first_diff..common {
            let l_bits = lhs[idx];
            let r_bits = rhs[idx];
            if obj_eq(_py, obj_from_bits(l_bits), obj_from_bits(r_bits)) {
                continue;
            }
            return compare_objects(_py, obj_from_bits(l_bits), obj_from_bits(r_bits));
        }
        CompareOutcome::Ordered(lhs.len().cmp(&rhs.len()))
    }
}

fn compare_objects_builtin(_py: &PyToken<'_>, lhs: MoltObject, rhs: MoltObject) -> CompareOutcome {
    match compare_numbers_outcome(lhs, rhs) {
        CompareOutcome::NotComparable => {}
        outcome => return outcome,
    }
    let (Some(lhs_ptr), Some(rhs_ptr)) = (lhs.as_ptr(), rhs.as_ptr()) else {
        return CompareOutcome::NotComparable;
    };
    unsafe {
        let ltype = object_type_id(lhs_ptr);
        let rtype = object_type_id(rhs_ptr);
        if ltype == TYPE_ID_STRING && rtype == TYPE_ID_STRING {
            return CompareOutcome::Ordered(compare_string_bytes(lhs_ptr, rhs_ptr));
        }
        if (ltype == TYPE_ID_BYTES || ltype == TYPE_ID_BYTEARRAY)
            && (rtype == TYPE_ID_BYTES || rtype == TYPE_ID_BYTEARRAY)
        {
            return CompareOutcome::Ordered(compare_bytes_like(lhs_ptr, rhs_ptr));
        }
        if ltype == TYPE_ID_LIST && rtype == TYPE_ID_LIST {
            return compare_sequence(_py, lhs_ptr, rhs_ptr);
        }
        if ltype == TYPE_ID_TUPLE && rtype == TYPE_ID_TUPLE {
            return compare_sequence(_py, lhs_ptr, rhs_ptr);
        }
    }
    CompareOutcome::NotComparable
}

fn ordering_matches(ordering: Ordering, op: CompareOp) -> bool {
    match op {
        CompareOp::Lt => ordering == Ordering::Less,
        CompareOp::Le => ordering != Ordering::Greater,
        CompareOp::Gt => ordering == Ordering::Greater,
        CompareOp::Ge => ordering != Ordering::Less,
    }
}

pub(crate) fn compare_builtin_bool(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
    op: CompareOp,
) -> CompareBoolOutcome {
    match compare_objects_builtin(_py, lhs, rhs) {
        CompareOutcome::Ordered(ordering) => {
            if ordering_matches(ordering, op) {
                CompareBoolOutcome::True
            } else {
                CompareBoolOutcome::False
            }
        }
        CompareOutcome::Unordered => CompareBoolOutcome::False,
        CompareOutcome::NotComparable => CompareBoolOutcome::NotComparable,
        CompareOutcome::Error => CompareBoolOutcome::Error,
    }
}

pub(crate) fn rich_compare_bool(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
    op_name_bits: u64,
    reverse_name_bits: u64,
) -> CompareBoolOutcome {
    let pending_before = exception_pending(_py);
    let prev_exc_bits = if pending_before {
        exception_last_bits_noinc(_py).unwrap_or(0)
    } else {
        0
    };
    let exception_changed = || {
        if !exception_pending(_py) {
            return false;
        }
        if !pending_before {
            return true;
        }
        exception_last_bits_noinc(_py).unwrap_or(0) != prev_exc_bits
    };
    if let Some(outcome) = rich_compare_type_bool(_py, lhs, rhs, op_name_bits, reverse_name_bits) {
        return outcome;
    }
    unsafe {
        if let Some(lhs_ptr) = lhs.as_ptr() {
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, lhs_ptr, op_name_bits) {
                let res_bits = call_callable1(_py, call_bits, rhs.bits());
                dec_ref_bits(_py, call_bits);
                if exception_changed() {
                    dec_ref_bits(_py, res_bits);
                    return CompareBoolOutcome::Error;
                }
                if is_not_implemented_bits(_py, res_bits) {
                    dec_ref_bits(_py, res_bits);
                } else {
                    let truthy = is_truthy(_py, obj_from_bits(res_bits));
                    dec_ref_bits(_py, res_bits);
                    return if truthy {
                        CompareBoolOutcome::True
                    } else {
                        CompareBoolOutcome::False
                    };
                }
            }
            if exception_changed() {
                return CompareBoolOutcome::Error;
            }
        }
        if let Some(rhs_ptr) = rhs.as_ptr() {
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, rhs_ptr, reverse_name_bits)
            {
                let res_bits = call_callable1(_py, call_bits, lhs.bits());
                dec_ref_bits(_py, call_bits);
                if exception_changed() {
                    dec_ref_bits(_py, res_bits);
                    return CompareBoolOutcome::Error;
                }
                if is_not_implemented_bits(_py, res_bits) {
                    dec_ref_bits(_py, res_bits);
                } else {
                    let truthy = is_truthy(_py, obj_from_bits(res_bits));
                    dec_ref_bits(_py, res_bits);
                    return if truthy {
                        CompareBoolOutcome::True
                    } else {
                        CompareBoolOutcome::False
                    };
                }
            }
            if exception_changed() {
                return CompareBoolOutcome::Error;
            }
        }
    }
    CompareBoolOutcome::NotComparable
}

fn rich_compare_value(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
    op_name_bits: u64,
    reverse_name_bits: u64,
) -> CompareValueOutcome {
    let pending_before = exception_pending(_py);
    let prev_exc_bits = if pending_before {
        exception_last_bits_noinc(_py).unwrap_or(0)
    } else {
        0
    };
    let exception_changed = || {
        if !exception_pending(_py) {
            return false;
        }
        if !pending_before {
            return true;
        }
        exception_last_bits_noinc(_py).unwrap_or(0) != prev_exc_bits
    };
    if let Some(outcome) = rich_compare_type_value(_py, lhs, rhs, op_name_bits, reverse_name_bits) {
        return outcome;
    }
    unsafe {
        if let Some(lhs_ptr) = lhs.as_ptr() {
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, lhs_ptr, op_name_bits) {
                let res_bits = call_callable1(_py, call_bits, rhs.bits());
                dec_ref_bits(_py, call_bits);
                if exception_changed() {
                    dec_ref_bits(_py, res_bits);
                    return CompareValueOutcome::Error;
                }
                if is_not_implemented_bits(_py, res_bits) {
                    dec_ref_bits(_py, res_bits);
                } else {
                    return CompareValueOutcome::Value(res_bits);
                }
            }
            if exception_changed() {
                return CompareValueOutcome::Error;
            }
        }
        if let Some(rhs_ptr) = rhs.as_ptr() {
            if let Some(call_bits) = attr_lookup_ptr_allow_missing(_py, rhs_ptr, reverse_name_bits)
            {
                let res_bits = call_callable1(_py, call_bits, lhs.bits());
                dec_ref_bits(_py, call_bits);
                if exception_changed() {
                    dec_ref_bits(_py, res_bits);
                    return CompareValueOutcome::Error;
                }
                if is_not_implemented_bits(_py, res_bits) {
                    dec_ref_bits(_py, res_bits);
                } else {
                    return CompareValueOutcome::Value(res_bits);
                }
            }
            if exception_changed() {
                return CompareValueOutcome::Error;
            }
        }
    }
    CompareValueOutcome::NotComparable
}

fn rich_compare_type_bool(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
    op_name_bits: u64,
    reverse_name_bits: u64,
) -> Option<CompareBoolOutcome> {
    unsafe {
        let mut saw_type = false;
        if let Some(lhs_ptr) = lhs.as_ptr()
            && object_type_id(lhs_ptr) == TYPE_ID_TYPE
        {
            saw_type = true;
            if let Some(outcome) = rich_compare_type_method(_py, lhs_ptr, rhs.bits(), op_name_bits)
            {
                return Some(outcome);
            }
        }
        if let Some(rhs_ptr) = rhs.as_ptr()
            && object_type_id(rhs_ptr) == TYPE_ID_TYPE
        {
            saw_type = true;
            if let Some(outcome) =
                rich_compare_type_method(_py, rhs_ptr, lhs.bits(), reverse_name_bits)
            {
                return Some(outcome);
            }
        }
        if saw_type {
            return Some(CompareBoolOutcome::NotComparable);
        }
    }
    None
}

fn rich_compare_type_value(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
    op_name_bits: u64,
    reverse_name_bits: u64,
) -> Option<CompareValueOutcome> {
    unsafe {
        let mut saw_type = false;
        if let Some(lhs_ptr) = lhs.as_ptr()
            && object_type_id(lhs_ptr) == TYPE_ID_TYPE
        {
            saw_type = true;
            if let Some(outcome) =
                rich_compare_type_method_value(_py, lhs_ptr, rhs.bits(), op_name_bits)
            {
                return Some(outcome);
            }
        }
        if let Some(rhs_ptr) = rhs.as_ptr()
            && object_type_id(rhs_ptr) == TYPE_ID_TYPE
        {
            saw_type = true;
            if let Some(outcome) =
                rich_compare_type_method_value(_py, rhs_ptr, lhs.bits(), reverse_name_bits)
            {
                return Some(outcome);
            }
        }
        if saw_type {
            return Some(CompareValueOutcome::NotComparable);
        }
    }
    None
}

unsafe fn rich_compare_type_method(
    _py: &PyToken<'_>,
    type_ptr: *mut u8,
    other_bits: u64,
    op_name_bits: u64,
) -> Option<CompareBoolOutcome> {
    unsafe {
        let pending_before = exception_pending(_py);
        let prev_exc_bits = if pending_before {
            exception_last_bits_noinc(_py).unwrap_or(0)
        } else {
            0
        };
        let exception_changed = || {
            if !exception_pending(_py) {
                return false;
            }
            if !pending_before {
                return true;
            }
            exception_last_bits_noinc(_py).unwrap_or(0) != prev_exc_bits
        };
        let mut meta_bits = object_class_bits(type_ptr);
        if meta_bits == 0 {
            meta_bits = builtin_classes(_py).type_obj;
        }
        let meta_ptr = match obj_from_bits(meta_bits).as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_TYPE => ptr,
            _ => return None,
        };
        let method_bits = class_attr_lookup_raw_mro(_py, meta_ptr, op_name_bits)?;
        let Some(bound_bits) = descriptor_bind(_py, method_bits, meta_ptr, Some(type_ptr)) else {
            if exception_changed() {
                return Some(CompareBoolOutcome::Error);
            }
            return None;
        };
        let res_bits = call_callable1(_py, bound_bits, other_bits);
        dec_ref_bits(_py, bound_bits);
        if exception_changed() {
            dec_ref_bits(_py, res_bits);
            return Some(CompareBoolOutcome::Error);
        }
        if is_not_implemented_bits(_py, res_bits) {
            dec_ref_bits(_py, res_bits);
            return None;
        }
        let truthy = is_truthy(_py, obj_from_bits(res_bits));
        dec_ref_bits(_py, res_bits);
        Some(if truthy {
            CompareBoolOutcome::True
        } else {
            CompareBoolOutcome::False
        })
    }
}

unsafe fn rich_compare_type_method_value(
    _py: &PyToken<'_>,
    type_ptr: *mut u8,
    other_bits: u64,
    op_name_bits: u64,
) -> Option<CompareValueOutcome> {
    unsafe {
        let pending_before = exception_pending(_py);
        let prev_exc_bits = if pending_before {
            exception_last_bits_noinc(_py).unwrap_or(0)
        } else {
            0
        };
        let exception_changed = || {
            if !exception_pending(_py) {
                return false;
            }
            if !pending_before {
                return true;
            }
            exception_last_bits_noinc(_py).unwrap_or(0) != prev_exc_bits
        };
        let mut meta_bits = object_class_bits(type_ptr);
        if meta_bits == 0 {
            meta_bits = builtin_classes(_py).type_obj;
        }
        let meta_ptr = match obj_from_bits(meta_bits).as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_TYPE => ptr,
            _ => return None,
        };
        let method_bits = class_attr_lookup_raw_mro(_py, meta_ptr, op_name_bits)?;
        let Some(bound_bits) = descriptor_bind(_py, method_bits, meta_ptr, Some(type_ptr)) else {
            if exception_changed() {
                return Some(CompareValueOutcome::Error);
            }
            return None;
        };
        let res_bits = call_callable1(_py, bound_bits, other_bits);
        dec_ref_bits(_py, bound_bits);
        if exception_changed() {
            dec_ref_bits(_py, res_bits);
            return Some(CompareValueOutcome::Error);
        }
        if is_not_implemented_bits(_py, res_bits) {
            dec_ref_bits(_py, res_bits);
            return None;
        }
        Some(CompareValueOutcome::Value(res_bits))
    }
}

fn rich_compare_order(_py: &PyToken<'_>, lhs: MoltObject, rhs: MoltObject) -> CompareOutcome {
    let lt_name_bits = intern_static_name(_py, &runtime_state(_py).interned.lt_name, b"__lt__");
    let gt_name_bits = intern_static_name(_py, &runtime_state(_py).interned.gt_name, b"__gt__");
    match rich_compare_bool(_py, lhs, rhs, lt_name_bits, gt_name_bits) {
        CompareBoolOutcome::True => return CompareOutcome::Ordered(Ordering::Less),
        CompareBoolOutcome::False => {}
        CompareBoolOutcome::NotComparable => return CompareOutcome::NotComparable,
        CompareBoolOutcome::Error => return CompareOutcome::Error,
    }
    match rich_compare_bool(_py, rhs, lhs, lt_name_bits, gt_name_bits) {
        CompareBoolOutcome::True => CompareOutcome::Ordered(Ordering::Greater),
        CompareBoolOutcome::False => CompareOutcome::Ordered(Ordering::Equal),
        CompareBoolOutcome::NotComparable => CompareOutcome::NotComparable,
        CompareBoolOutcome::Error => CompareOutcome::Error,
    }
}

pub(crate) fn compare_objects(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
) -> CompareOutcome {
    match compare_objects_builtin(_py, lhs, rhs) {
        CompareOutcome::NotComparable => {}
        outcome => return outcome,
    }
    rich_compare_order(_py, lhs, rhs)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_lt(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        match compare_builtin_bool(_py, lhs, rhs, CompareOp::Lt) {
            CompareBoolOutcome::True => return MoltObject::from_bool(true).bits(),
            CompareBoolOutcome::False => return MoltObject::from_bool(false).bits(),
            CompareBoolOutcome::Error => return MoltObject::none().bits(),
            CompareBoolOutcome::NotComparable => {}
        }
        let lt_name_bits = intern_static_name(_py, &runtime_state(_py).interned.lt_name, b"__lt__");
        let gt_name_bits = intern_static_name(_py, &runtime_state(_py).interned.gt_name, b"__gt__");
        match rich_compare_value(_py, lhs, rhs, lt_name_bits, gt_name_bits) {
            CompareValueOutcome::Value(bits) => bits,
            CompareValueOutcome::Error => MoltObject::none().bits(),
            CompareValueOutcome::NotComparable => compare_type_error(_py, lhs, rhs, "<"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_le(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        match compare_builtin_bool(_py, lhs, rhs, CompareOp::Le) {
            CompareBoolOutcome::True => return MoltObject::from_bool(true).bits(),
            CompareBoolOutcome::False => return MoltObject::from_bool(false).bits(),
            CompareBoolOutcome::Error => return MoltObject::none().bits(),
            CompareBoolOutcome::NotComparable => {}
        }
        let le_name_bits = intern_static_name(_py, &runtime_state(_py).interned.le_name, b"__le__");
        let ge_name_bits = intern_static_name(_py, &runtime_state(_py).interned.ge_name, b"__ge__");
        match rich_compare_value(_py, lhs, rhs, le_name_bits, ge_name_bits) {
            CompareValueOutcome::Value(bits) => bits,
            CompareValueOutcome::Error => MoltObject::none().bits(),
            CompareValueOutcome::NotComparable => compare_type_error(_py, lhs, rhs, "<="),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gt(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        match compare_builtin_bool(_py, lhs, rhs, CompareOp::Gt) {
            CompareBoolOutcome::True => return MoltObject::from_bool(true).bits(),
            CompareBoolOutcome::False => return MoltObject::from_bool(false).bits(),
            CompareBoolOutcome::Error => return MoltObject::none().bits(),
            CompareBoolOutcome::NotComparable => {}
        }
        let gt_name_bits = intern_static_name(_py, &runtime_state(_py).interned.gt_name, b"__gt__");
        let lt_name_bits = intern_static_name(_py, &runtime_state(_py).interned.lt_name, b"__lt__");
        match rich_compare_value(_py, lhs, rhs, gt_name_bits, lt_name_bits) {
            CompareValueOutcome::Value(bits) => bits,
            CompareValueOutcome::Error => MoltObject::none().bits(),
            CompareValueOutcome::NotComparable => compare_type_error(_py, lhs, rhs, ">"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ge(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        match compare_builtin_bool(_py, lhs, rhs, CompareOp::Ge) {
            CompareBoolOutcome::True => return MoltObject::from_bool(true).bits(),
            CompareBoolOutcome::False => return MoltObject::from_bool(false).bits(),
            CompareBoolOutcome::Error => return MoltObject::none().bits(),
            CompareBoolOutcome::NotComparable => {}
        }
        let ge_name_bits = intern_static_name(_py, &runtime_state(_py).interned.ge_name, b"__ge__");
        let le_name_bits = intern_static_name(_py, &runtime_state(_py).interned.le_name, b"__le__");
        match rich_compare_value(_py, lhs, rhs, ge_name_bits, le_name_bits) {
            CompareValueOutcome::Value(bits) => bits,
            CompareValueOutcome::Error => MoltObject::none().bits(),
            CompareValueOutcome::NotComparable => compare_type_error(_py, lhs, rhs, ">="),
        }
    })
}

#[inline]
fn trace_eq_enabled() -> bool {
    static ENABLED: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ENABLED.get_or_init(|| std::env::var("MOLT_TRACE_EQ").is_ok())
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_eq(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if trace_eq_enabled() {
            eprintln!("molt_eq: a=0x{:016x} b=0x{:016x}", a, b);
        }
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        let ellipsis = ellipsis_bits(_py);
        if a == ellipsis || b == ellipsis {
            return MoltObject::from_bool(a == b).bits();
        }
        let eq_name_bits = intern_static_name(_py, &runtime_state(_py).interned.eq_name, b"__eq__");
        match rich_compare_value(_py, lhs, rhs, eq_name_bits, eq_name_bits) {
            CompareValueOutcome::Value(bits) => return bits,
            CompareValueOutcome::Error => return MoltObject::none().bits(),
            CompareValueOutcome::NotComparable => {}
        }
        MoltObject::from_bool(obj_eq(_py, lhs, rhs)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ne(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        match compare_objects_builtin(_py, lhs, rhs) {
            CompareOutcome::Ordered(ordering) => {
                return MoltObject::from_bool(ordering != Ordering::Equal).bits();
            }
            CompareOutcome::Unordered => return MoltObject::from_bool(true).bits(),
            CompareOutcome::Error => return MoltObject::none().bits(),
            CompareOutcome::NotComparable => {}
        }
        let lhs_type_bits = type_of_bits(_py, a);
        let rhs_type_bits = type_of_bits(_py, b);
        let lhs_type_ptr = obj_from_bits(lhs_type_bits).as_ptr();
        let rhs_type_ptr = obj_from_bits(rhs_type_bits).as_ptr();
        let ne_name_bits = intern_static_name(_py, &runtime_state(_py).interned.ne_name, b"__ne__");
        let object_ne_raw = unsafe {
            obj_from_bits(builtin_classes(_py).object)
                .as_ptr()
                .and_then(|ptr| class_attr_lookup_raw_mro(_py, ptr, ne_name_bits))
        };
        let lhs_ne_raw = unsafe {
            lhs_type_ptr.and_then(|ptr| class_attr_lookup_raw_mro(_py, ptr, ne_name_bits))
        };
        let rhs_ne_raw = unsafe {
            rhs_type_ptr.and_then(|ptr| class_attr_lookup_raw_mro(_py, ptr, ne_name_bits))
        };
        let lhs_ne_is_object_default = lhs_ne_raw.is_some() && lhs_ne_raw == object_ne_raw;

        let mut lhs_ne_notimplemented_or_missing = true;
        if let (Some(lhs_ptr), Some(lhs_tp), Some(lhs_raw)) =
            (lhs.as_ptr(), lhs_type_ptr, lhs_ne_raw)
        {
            unsafe {
                match call_dunder_raw(_py, lhs_raw, lhs_tp, Some(lhs_ptr), b) {
                    BinaryDunderOutcome::Value(bits) => return bits,
                    BinaryDunderOutcome::Error => return MoltObject::none().bits(),
                    BinaryDunderOutcome::NotImplemented | BinaryDunderOutcome::Missing => {
                        lhs_ne_notimplemented_or_missing = true;
                    }
                }
            }
        }

        if lhs_ne_notimplemented_or_missing {
            let rhs_is_subclass =
                rhs_type_bits != lhs_type_bits && issubclass_bits(rhs_type_bits, lhs_type_bits);
            let rhs_has_custom_ne = rhs_ne_raw.is_some() && rhs_ne_raw != object_ne_raw;
            let rhs_differs_from_lhs = lhs_ne_raw.is_none_or(|lhs_raw| Some(lhs_raw) != rhs_ne_raw);
            let should_call_rhs = rhs_type_bits != lhs_type_bits
                && rhs_has_custom_ne
                && (rhs_is_subclass || lhs_ne_raw.is_none() || rhs_differs_from_lhs);
            if should_call_rhs
                && let (Some(rhs_ptr), Some(rhs_tp), Some(rhs_raw)) =
                    (rhs.as_ptr(), rhs_type_ptr, rhs_ne_raw)
            {
                unsafe {
                    match call_dunder_raw(_py, rhs_raw, rhs_tp, Some(rhs_ptr), a) {
                        BinaryDunderOutcome::Value(bits) => return bits,
                        BinaryDunderOutcome::Error => return MoltObject::none().bits(),
                        BinaryDunderOutcome::NotImplemented | BinaryDunderOutcome::Missing => {}
                    }
                }
            }
        }

        if lhs_ne_is_object_default || lhs_ne_raw.is_none() {
            let eq_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.eq_name, b"__eq__");
            match rich_compare_value(_py, lhs, rhs, eq_name_bits, eq_name_bits) {
                CompareValueOutcome::Value(bits) => {
                    let truthy = is_truthy(_py, obj_from_bits(bits));
                    let had_exc = exception_pending(_py);
                    dec_ref_bits(_py, bits);
                    if had_exc {
                        return MoltObject::none().bits();
                    }
                    return MoltObject::from_bool(!truthy).bits();
                }
                CompareValueOutcome::Error => return MoltObject::none().bits(),
                CompareValueOutcome::NotComparable => {}
            }
        }

        MoltObject::from_bool(a != b).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_string_eq(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        let Some(lp) = lhs.as_ptr() else {
            return MoltObject::from_bool(false).bits();
        };
        let Some(rp) = rhs.as_ptr() else {
            return MoltObject::from_bool(false).bits();
        };
        unsafe {
            if object_type_id(lp) != TYPE_ID_STRING || object_type_id(rp) != TYPE_ID_STRING {
                return MoltObject::from_bool(false).bits();
            }
            if lp == rp {
                return MoltObject::from_bool(true).bits();
            }
            let l_len = string_len(lp);
            let r_len = string_len(rp);
            if l_len != r_len {
                return MoltObject::from_bool(false).bits();
            }
            MoltObject::from_bool(simd_bytes_eq(string_bytes(lp), string_bytes(rp), l_len)).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_is(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { MoltObject::from_bool(a == b).bits() })
}
