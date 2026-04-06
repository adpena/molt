//! Container reference optimization (Monty-inspired `contains_refs`).
//!
//! Every list, dict, tuple, and set tracks a `HEADER_FLAG_CONTAINS_REFS` bit
//! in its `MoltHeader.flags`.  When the container holds only primitive values
//! (ints, floats, bools, None) — which is the common case — `dec_ref` cleanup
//! can skip iterating over elements entirely.  This turns an O(n) scan into an
//! O(1) flag check for the majority of containers.
//!
//! A value "contains a ref" if and only if it is a NaN-boxed pointer (`TAG_PTR`).

use molt_obj_model::MoltObject;

/// Returns `true` if the NaN-boxed `bits` value is a heap pointer that
/// participates in reference counting.
#[inline(always)]
pub(crate) fn is_heap_ref(bits: u64) -> bool {
    MoltObject::from_bits(bits).is_ptr()
}

/// Returns `true` if any value in `values` is a heap pointer.
///
/// When this returns `false`, the entire slice consists of primitives
/// (int, float, bool, None) and no refcount work is needed.
#[inline]
pub(crate) fn slice_contains_heap_refs(values: &[u64]) -> bool {
    values.iter().any(|&bits| is_heap_ref(bits))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::PI;

    #[test]
    fn primitive_int_is_not_heap_ref() {
        let obj = MoltObject::from_int(42);
        assert!(!is_heap_ref(obj.bits()));
    }

    #[test]
    fn primitive_float_is_not_heap_ref() {
        let obj = MoltObject::from_float(PI);
        assert!(!is_heap_ref(obj.bits()));
    }

    #[test]
    fn primitive_bool_is_not_heap_ref() {
        assert!(!is_heap_ref(MoltObject::from_bool(true).bits()));
        assert!(!is_heap_ref(MoltObject::from_bool(false).bits()));
    }

    #[test]
    fn none_is_not_heap_ref() {
        assert!(!is_heap_ref(MoltObject::none().bits()));
    }

    #[test]
    fn pointer_is_heap_ref() {
        // Allocate a small block to get a valid pointer.
        let layout = std::alloc::Layout::from_size_align(8, 8).unwrap();
        let ptr = unsafe { std::alloc::alloc(layout) };
        assert!(!ptr.is_null());
        let obj = MoltObject::from_ptr(ptr);
        assert!(is_heap_ref(obj.bits()));
        unsafe { std::alloc::dealloc(ptr, layout) };
    }

    #[test]
    fn slice_all_primitives() {
        let values = [
            MoltObject::from_int(1).bits(),
            MoltObject::from_float(2.5).bits(),
            MoltObject::from_bool(true).bits(),
            MoltObject::none().bits(),
        ];
        assert!(!slice_contains_heap_refs(&values));
    }

    #[test]
    fn slice_with_one_pointer() {
        let layout = std::alloc::Layout::from_size_align(8, 8).unwrap();
        let ptr = unsafe { std::alloc::alloc(layout) };
        let values = [
            MoltObject::from_int(1).bits(),
            MoltObject::from_ptr(ptr).bits(),
            MoltObject::from_int(3).bits(),
        ];
        assert!(slice_contains_heap_refs(&values));
        unsafe { std::alloc::dealloc(ptr, layout) };
    }

    #[test]
    fn empty_slice_has_no_refs() {
        assert!(!slice_contains_heap_refs(&[]));
    }
}
