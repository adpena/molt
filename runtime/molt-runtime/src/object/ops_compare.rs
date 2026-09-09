// Comparison operations and helpers.
// Split from ops.rs for compilation-unit size reduction.

use crate::*;
use molt_obj_model::MoltObject;
use std::cmp::Ordering;

use super::ops::{is_float_extended, simd_bytes_eq, simd_find_first_mismatch};

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
pub(crate) enum CompareValueOutcome {
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

fn builtin_comparison_operands(_py: &PyToken<'_>, lhs: MoltObject, rhs: MoltObject) -> bool {
    [lhs, rhs].into_iter().all(|value| {
        let Some(ptr) = value.as_ptr() else {
            return true;
        };
        let class = unsafe { object_class_bits(ptr) };
        class == 0 || is_builtin_class_bits(_py, class)
    })
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
    unsafe {
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
}

#[cfg(target_arch = "x86_64")]
#[inline]
unsafe fn simd_find_first_byte_diff_avx2(a: *const u8, b: *const u8, len: usize) -> usize {
    unsafe {
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
    // Three-way ordering consumers use the same value-producing sequence
    // contract as the six Python operators; only this boundary consumes truth.
    unsafe {
        match comparison_value_to_bool(
            _py,
            compare_sequence_value(_py, lhs_ptr, rhs_ptr, CompareOp::Lt),
        ) {
            CompareBoolOutcome::True => return CompareOutcome::Ordered(Ordering::Less),
            CompareBoolOutcome::False => {}
            CompareBoolOutcome::Error => return CompareOutcome::Error,
            CompareBoolOutcome::NotComparable => return CompareOutcome::NotComparable,
        }
        match comparison_value_to_bool(
            _py,
            compare_sequence_value(_py, rhs_ptr, lhs_ptr, CompareOp::Lt),
        ) {
            CompareBoolOutcome::True => CompareOutcome::Ordered(Ordering::Greater),
            CompareBoolOutcome::False => CompareOutcome::Ordered(Ordering::Equal),
            CompareBoolOutcome::Error => CompareOutcome::Error,
            CompareBoolOutcome::NotComparable => CompareOutcome::NotComparable,
        }
    }
}

struct ComparisonRecursionGuard;

impl ComparisonRecursionGuard {
    fn enter(_py: &PyToken<'_>) -> Option<Self> {
        if crate::state::recursion::recursion_guard_enter_fast() {
            Some(Self)
        } else {
            raise_exception::<u64>(
                _py,
                "RecursionError",
                "maximum recursion depth exceeded in comparison",
            );
            None
        }
    }
}

impl Drop for ComparisonRecursionGuard {
    fn drop(&mut self) {
        crate::state::recursion::recursion_guard_exit_fast();
    }
}

unsafe fn compare_sequence_eq_bool(
    _py: &PyToken<'_>,
    lhs_ptr: *mut u8,
    rhs_ptr: *mut u8,
) -> CompareBoolOutcome {
    unsafe {
        let Some(_guard) = ComparisonRecursionGuard::enter(_py) else {
            return CompareBoolOutcome::Error;
        };
        if object_type_id(lhs_ptr) == TYPE_ID_TUPLE {
            return crate::object::seq_access::with_immutable_tuple_slice(lhs_ptr, |lhs| {
                crate::object::seq_access::with_immutable_tuple_slice(rhs_ptr, |rhs| {
                    if lhs.len() != rhs.len() {
                        return CompareBoolOutcome::False;
                    }
                    let first_diff = simd_find_first_mismatch(lhs, rhs);
                    for idx in first_diff..lhs.len() {
                        let l_bits = lhs[idx];
                        let r_bits = rhs[idx];
                        if l_bits == r_bits {
                            continue;
                        }
                        match compare_object_eq_bool(
                            _py,
                            obj_from_bits(l_bits),
                            obj_from_bits(r_bits),
                        ) {
                            CompareBoolOutcome::True => {}
                            CompareBoolOutcome::False | CompareBoolOutcome::NotComparable => {
                                return CompareBoolOutcome::False;
                            }
                            CompareBoolOutcome::Error => {
                                return CompareBoolOutcome::Error;
                            }
                        }
                    }
                    CompareBoolOutcome::True
                })
                .unwrap_or(CompareBoolOutcome::Error)
            })
            .unwrap_or(CompareBoolOutcome::Error);
        }

        if crate::object::seq_access::locked_len(lhs_ptr)
            != crate::object::seq_access::locked_len(rhs_ptr)
        {
            return CompareBoolOutcome::False;
        }
        let mut idx = 0;
        loop {
            let lhs_len = crate::object::seq_access::locked_len(lhs_ptr);
            let rhs_len = crate::object::seq_access::locked_len(rhs_ptr);
            if idx >= lhs_len.min(rhs_len) {
                return if lhs_len == rhs_len {
                    CompareBoolOutcome::True
                } else {
                    CompareBoolOutcome::False
                };
            }
            let Some(lhs) = crate::object::seq_access::pin_item(_py, lhs_ptr, idx) else {
                continue;
            };
            let Some(rhs) = crate::object::seq_access::pin_item(_py, rhs_ptr, idx) else {
                continue;
            };
            let l_bits = lhs.bits();
            let r_bits = rhs.bits();
            if l_bits != r_bits {
                match compare_object_eq_bool(_py, obj_from_bits(l_bits), obj_from_bits(r_bits)) {
                    CompareBoolOutcome::True => {}
                    CompareBoolOutcome::False | CompareBoolOutcome::NotComparable => {
                        return CompareBoolOutcome::False;
                    }
                    CompareBoolOutcome::Error => {
                        return CompareBoolOutcome::Error;
                    }
                }
            }
            idx += 1;
        }
    }
}

fn compare_builtin_eq_bool(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
) -> CompareBoolOutcome {
    if !builtin_comparison_operands(_py, lhs, rhs) {
        return CompareBoolOutcome::NotComparable;
    }
    match compare_numbers_outcome(lhs, rhs) {
        CompareOutcome::Ordered(ordering) => {
            return if ordering == Ordering::Equal {
                CompareBoolOutcome::True
            } else {
                CompareBoolOutcome::False
            };
        }
        CompareOutcome::Unordered => return CompareBoolOutcome::False,
        CompareOutcome::Error => return CompareBoolOutcome::Error,
        CompareOutcome::NotComparable => {}
    }
    if lhs.is_none() && rhs.is_none() {
        return CompareBoolOutcome::True;
    }
    let (Some(lhs_ptr), Some(rhs_ptr)) = (lhs.as_ptr(), rhs.as_ptr()) else {
        return CompareBoolOutcome::NotComparable;
    };
    unsafe {
        let ltype = object_type_id(lhs_ptr);
        let rtype = object_type_id(rhs_ptr);
        if (ltype == TYPE_ID_LIST && rtype == TYPE_ID_LIST)
            || (ltype == TYPE_ID_TUPLE && rtype == TYPE_ID_TUPLE)
        {
            return compare_sequence_eq_bool(_py, lhs_ptr, rhs_ptr);
        }
        if ltype == TYPE_ID_STRING && rtype == TYPE_ID_STRING {
            return if compare_string_bytes(lhs_ptr, rhs_ptr) == Ordering::Equal {
                CompareBoolOutcome::True
            } else {
                CompareBoolOutcome::False
            };
        }
        if (ltype == TYPE_ID_BYTES || ltype == TYPE_ID_BYTEARRAY)
            && (rtype == TYPE_ID_BYTES || rtype == TYPE_ID_BYTEARRAY)
        {
            return if compare_bytes_like(lhs_ptr, rhs_ptr) == Ordering::Equal {
                CompareBoolOutcome::True
            } else {
                CompareBoolOutcome::False
            };
        }
        if (is_set_like_type(ltype) || is_set_view_type(ltype))
            && (is_set_like_type(rtype) || is_set_view_type(rtype))
        {
            return if obj_eq(_py, lhs, rhs) {
                CompareBoolOutcome::True
            } else {
                CompareBoolOutcome::False
            };
        }
    }
    CompareBoolOutcome::NotComparable
}

/// Consume an owned rich-comparison result only at a truth-valued boundary.
pub(crate) fn comparison_value_to_bool(
    _py: &PyToken<'_>,
    outcome: CompareValueOutcome,
) -> CompareBoolOutcome {
    let bits = match outcome {
        CompareValueOutcome::Value(bits) => bits,
        CompareValueOutcome::NotComparable => return CompareBoolOutcome::NotComparable,
        CompareValueOutcome::Error => return CompareBoolOutcome::Error,
    };
    let previous = exception_last_bits_noinc(_py);
    let truthy = is_truthy(_py, obj_from_bits(bits));
    dec_ref_bits(_py, bits);
    if exception_pending(_py) && exception_last_bits_noinc(_py) != previous {
        CompareBoolOutcome::Error
    } else if truthy {
        CompareBoolOutcome::True
    } else {
        CompareBoolOutcome::False
    }
}

fn comparison_bool_to_value(outcome: CompareBoolOutcome) -> CompareValueOutcome {
    match outcome {
        CompareBoolOutcome::True => CompareValueOutcome::Value(MoltObject::from_bool(true).bits()),
        CompareBoolOutcome::False => {
            CompareValueOutcome::Value(MoltObject::from_bool(false).bits())
        }
        CompareBoolOutcome::NotComparable => CompareValueOutcome::NotComparable,
        CompareBoolOutcome::Error => CompareValueOutcome::Error,
    }
}

fn compare_object_eq_value(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
) -> CompareValueOutcome {
    match compare_builtin_eq_bool(_py, lhs, rhs) {
        CompareBoolOutcome::NotComparable => {}
        outcome => return comparison_bool_to_value(outcome),
    }
    let name = intern_static_name(_py, &runtime_state(_py).interned.eq_name, b"__eq__");
    match rich_compare_value(_py, lhs, rhs, name, name) {
        CompareValueOutcome::NotComparable => {
            let previous = exception_last_bits_noinc(_py);
            let equal = obj_eq(_py, lhs, rhs);
            if exception_pending(_py) && exception_last_bits_noinc(_py) != previous {
                CompareValueOutcome::Error
            } else {
                CompareValueOutcome::Value(MoltObject::from_bool(equal).bits())
            }
        }
        outcome => outcome,
    }
}

pub(crate) fn compare_object_eq_bool(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
) -> CompareBoolOutcome {
    // Container element comparison uses Python's identity-or-equality contract;
    // the value-producing equality operation deliberately has no such shortcut.
    if lhs.bits() == rhs.bits() {
        return CompareBoolOutcome::True;
    }
    comparison_value_to_bool(_py, compare_object_eq_value(_py, lhs, rhs))
}

fn compare_objects_builtin(_py: &PyToken<'_>, lhs: MoltObject, rhs: MoltObject) -> CompareOutcome {
    if !builtin_comparison_operands(_py, lhs, rhs) {
        return CompareOutcome::NotComparable;
    }
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

fn compare_op_symbol(op: CompareOp) -> &'static str {
    match op {
        CompareOp::Lt => "<",
        CompareOp::Le => "<=",
        CompareOp::Gt => ">",
        CompareOp::Ge => ">=",
    }
}

fn compare_op_method_names(_py: &PyToken<'_>, op: CompareOp) -> (u64, u64) {
    match op {
        CompareOp::Lt => (
            intern_static_name(_py, &runtime_state(_py).interned.lt_name, b"__lt__"),
            intern_static_name(_py, &runtime_state(_py).interned.gt_name, b"__gt__"),
        ),
        CompareOp::Le => (
            intern_static_name(_py, &runtime_state(_py).interned.le_name, b"__le__"),
            intern_static_name(_py, &runtime_state(_py).interned.ge_name, b"__ge__"),
        ),
        CompareOp::Gt => (
            intern_static_name(_py, &runtime_state(_py).interned.gt_name, b"__gt__"),
            intern_static_name(_py, &runtime_state(_py).interned.lt_name, b"__lt__"),
        ),
        CompareOp::Ge => (
            intern_static_name(_py, &runtime_state(_py).interned.ge_name, b"__ge__"),
            intern_static_name(_py, &runtime_state(_py).interned.le_name, b"__le__"),
        ),
    }
}

fn compare_object_value_for_op(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
    op: CompareOp,
) -> CompareValueOutcome {
    match compare_builtin_value(_py, lhs, rhs, op) {
        CompareValueOutcome::NotComparable => {}
        outcome => return outcome,
    }
    let (name, reflected) = compare_op_method_names(_py, op);
    match rich_compare_value(_py, lhs, rhs, name, reflected) {
        CompareValueOutcome::NotComparable => {
            compare_type_error(_py, lhs, rhs, compare_op_symbol(op));
            CompareValueOutcome::Error
        }
        outcome => outcome,
    }
}

unsafe fn compare_sequence_value(
    _py: &PyToken<'_>,
    lhs_ptr: *mut u8,
    rhs_ptr: *mut u8,
    op: CompareOp,
) -> CompareValueOutcome {
    let Some(_guard) = ComparisonRecursionGuard::enter(_py) else {
        return CompareValueOutcome::Error;
    };
    unsafe {
        if object_type_id(lhs_ptr) == TYPE_ID_TUPLE {
            return crate::object::seq_access::with_immutable_tuple_slice(lhs_ptr, |lhs| {
                crate::object::seq_access::with_immutable_tuple_slice(rhs_ptr, |rhs| {
                    let common = lhs.len().min(rhs.len());
                    let first_diff = simd_find_first_mismatch(lhs, rhs);
                    for idx in first_diff..common {
                        let l_bits = lhs[idx];
                        let r_bits = rhs[idx];
                        match compare_object_eq_bool(
                            _py,
                            obj_from_bits(l_bits),
                            obj_from_bits(r_bits),
                        ) {
                            CompareBoolOutcome::True => continue,
                            CompareBoolOutcome::False | CompareBoolOutcome::NotComparable => {}
                            CompareBoolOutcome::Error => return CompareValueOutcome::Error,
                        }
                        return compare_object_value_for_op(
                            _py,
                            obj_from_bits(l_bits),
                            obj_from_bits(r_bits),
                            op,
                        );
                    }
                    CompareValueOutcome::Value(
                        MoltObject::from_bool(ordering_matches(lhs.len().cmp(&rhs.len()), op))
                            .bits(),
                    )
                })
                .unwrap_or(CompareValueOutcome::Error)
            })
            .unwrap_or(CompareValueOutcome::Error);
        }

        let mut idx = 0;
        loop {
            let lhs_len = crate::object::seq_access::locked_len(lhs_ptr);
            let rhs_len = crate::object::seq_access::locked_len(rhs_ptr);
            if idx >= lhs_len.min(rhs_len) {
                return CompareValueOutcome::Value(
                    MoltObject::from_bool(ordering_matches(lhs_len.cmp(&rhs_len), op)).bits(),
                );
            }
            let Some(lhs) = crate::object::seq_access::pin_item(_py, lhs_ptr, idx) else {
                continue;
            };
            let Some(rhs) = crate::object::seq_access::pin_item(_py, rhs_ptr, idx) else {
                continue;
            };
            let l_bits = lhs.bits();
            let r_bits = rhs.bits();
            match compare_object_eq_bool(_py, obj_from_bits(l_bits), obj_from_bits(r_bits)) {
                CompareBoolOutcome::True => {
                    idx += 1;
                    continue;
                }
                CompareBoolOutcome::False | CompareBoolOutcome::NotComparable => {}
                CompareBoolOutcome::Error => return CompareValueOutcome::Error,
            }
            return compare_object_value_for_op(
                _py,
                obj_from_bits(l_bits),
                obj_from_bits(r_bits),
                op,
            );
        }
    }
}

pub(crate) fn compare_builtin_bool(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
    op: CompareOp,
) -> CompareBoolOutcome {
    comparison_value_to_bool(_py, compare_builtin_value(_py, lhs, rhs, op))
}

fn compare_builtin_value(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
    op: CompareOp,
) -> CompareValueOutcome {
    if !builtin_comparison_operands(_py, lhs, rhs) {
        return CompareValueOutcome::NotComparable;
    }
    if let (Some(lhs_ptr), Some(rhs_ptr)) = (lhs.as_ptr(), rhs.as_ptr()) {
        unsafe {
            let ltype = object_type_id(lhs_ptr);
            let rtype = object_type_id(rhs_ptr);
            if (ltype == TYPE_ID_LIST && rtype == TYPE_ID_LIST)
                || (ltype == TYPE_ID_TUPLE && rtype == TYPE_ID_TUPLE)
            {
                return compare_sequence_value(_py, lhs_ptr, rhs_ptr, op);
            }
        }
    }
    comparison_bool_to_value(match compare_objects_builtin(_py, lhs, rhs) {
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
    })
}

pub(crate) fn rich_compare_bool(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
    op_name_bits: u64,
    reverse_name_bits: u64,
) -> CompareBoolOutcome {
    comparison_value_to_bool(
        _py,
        rich_compare_value(_py, lhs, rhs, op_name_bits, reverse_name_bits),
    )
}

/// Invoke one type slot, never an instance attribute. This also serves
/// object.__ne__, which delegates to its receiver's __eq__ slot, not a second
/// complete reflected comparison.
pub(crate) fn rich_compare_method_value(
    _py: &PyToken<'_>,
    receiver: MoltObject,
    other: MoltObject,
    name: u64,
) -> CompareValueOutcome {
    let Some(class) = obj_from_bits(type_of_bits(_py, receiver.bits())).as_ptr() else {
        return CompareValueOutcome::NotComparable;
    };
    let previous = exception_last_bits_noinc(_py);
    let changed = || exception_pending(_py) && exception_last_bits_noinc(_py) != previous;
    unsafe {
        let Some(raw) = class_attr_lookup_raw_mro(_py, class, name) else {
            return if changed() {
                CompareValueOutcome::Error
            } else {
                CompareValueOutcome::NotComparable
            };
        };
        let result = if let Some(instance) = receiver.as_ptr() {
            let Some(bound) = descriptor_bind(_py, raw, class, Some(instance)) else {
                return if changed() {
                    CompareValueOutcome::Error
                } else {
                    CompareValueOutcome::NotComparable
                };
            };
            let result = call_callable1(_py, bound, other.bits());
            dec_ref_bits(_py, bound);
            result
        } else {
            // Immediate builtins have no instance pointer to bind. Their type
            // slots are immutable builtin callables with explicit self.
            call_callable2(_py, raw, receiver.bits(), other.bits())
        };
        if changed() {
            dec_ref_bits(_py, result);
            return CompareValueOutcome::Error;
        }
        if is_not_implemented_bits(_py, result) {
            dec_ref_bits(_py, result);
            CompareValueOutcome::NotComparable
        } else {
            CompareValueOutcome::Value(result)
        }
    }
}

pub(crate) fn rich_compare_value(
    _py: &PyToken<'_>,
    lhs: MoltObject,
    rhs: MoltObject,
    op_name_bits: u64,
    reverse_name_bits: u64,
) -> CompareValueOutcome {
    let left_class = type_of_bits(_py, lhs.bits());
    let right_class = type_of_bits(_py, rhs.bits());
    // Rich comparisons give a strict subtype priority even when its method is
    // inherited unchanged. Arithmetic's different-implementation gate is wrong here.
    let reflected_first = left_class != right_class && issubclass_bits(right_class, left_class);
    if reflected_first {
        match rich_compare_method_value(_py, rhs, lhs, reverse_name_bits) {
            CompareValueOutcome::NotComparable => {}
            outcome => return outcome,
        }
    }
    match rich_compare_method_value(_py, lhs, rhs, op_name_bits) {
        CompareValueOutcome::NotComparable => {}
        outcome => return outcome,
    }
    // Equal types still receive a reflected attempt after NotImplemented.
    if !reflected_first {
        return rich_compare_method_value(_py, rhs, lhs, reverse_name_bits);
    }
    CompareValueOutcome::NotComparable
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
        match compare_object_value_for_op(_py, obj_from_bits(a), obj_from_bits(b), CompareOp::Lt) {
            CompareValueOutcome::Value(bits) => bits,
            CompareValueOutcome::Error | CompareValueOutcome::NotComparable => {
                MoltObject::none().bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_le(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match compare_object_value_for_op(_py, obj_from_bits(a), obj_from_bits(b), CompareOp::Le) {
            CompareValueOutcome::Value(bits) => bits,
            CompareValueOutcome::Error | CompareValueOutcome::NotComparable => {
                MoltObject::none().bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_gt(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match compare_object_value_for_op(_py, obj_from_bits(a), obj_from_bits(b), CompareOp::Gt) {
            CompareValueOutcome::Value(bits) => bits,
            CompareValueOutcome::Error | CompareValueOutcome::NotComparable => {
                MoltObject::none().bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ge(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match compare_object_value_for_op(_py, obj_from_bits(a), obj_from_bits(b), CompareOp::Ge) {
            CompareValueOutcome::Value(bits) => bits,
            CompareValueOutcome::Error | CompareValueOutcome::NotComparable => {
                MoltObject::none().bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_eq(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match compare_object_eq_value(_py, obj_from_bits(a), obj_from_bits(b)) {
            CompareValueOutcome::Value(bits) => bits,
            CompareValueOutcome::Error | CompareValueOutcome::NotComparable => {
                MoltObject::none().bits()
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_ne(a: u64, b: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let lhs = obj_from_bits(a);
        let rhs = obj_from_bits(b);
        match compare_builtin_eq_bool(_py, lhs, rhs) {
            CompareBoolOutcome::True => return MoltObject::from_bool(false).bits(),
            CompareBoolOutcome::False => return MoltObject::from_bool(true).bits(),
            CompareBoolOutcome::Error => return MoltObject::none().bits(),
            CompareBoolOutcome::NotComparable => {}
        }
        let name = intern_static_name(_py, &runtime_state(_py).interned.ne_name, b"__ne__");
        match rich_compare_value(_py, lhs, rhs, name, name) {
            CompareValueOutcome::Value(bits) => bits,
            CompareValueOutcome::Error => MoltObject::none().bits(),
            CompareValueOutcome::NotComparable => {
                let previous = exception_last_bits_noinc(_py);
                let equal = obj_eq(_py, lhs, rhs);
                if exception_pending(_py) && exception_last_bits_noinc(_py) != previous {
                    MoltObject::none().bits()
                } else {
                    MoltObject::from_bool(!equal).bits()
                }
            }
        }
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
