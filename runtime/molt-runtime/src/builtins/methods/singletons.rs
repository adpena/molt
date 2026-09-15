use crate::PyToken;
use crate::object::builders::{CanonicalSpecialSingleton, canonical_special_singleton_bits};
use crate::*;

pub(crate) fn missing_bits(_py: &PyToken<'_>) -> u64 {
    canonical_special_singleton_bits(_py, CanonicalSpecialSingleton::Missing)
}

pub(crate) fn is_missing_bits(_py: &PyToken<'_>, bits: u64) -> bool {
    if bits == missing_bits(_py) {
        return true;
    }
    let Some(ptr) = maybe_ptr_from_bits(bits) else {
        return false;
    };
    unsafe {
        object_type_id(ptr) == TYPE_ID_OBJECT
            && object_class_bits(ptr) == 0
            && object_payload_size(ptr) == 0
    }
}

pub(crate) fn not_implemented_bits(_py: &PyToken<'_>) -> u64 {
    canonical_special_singleton_bits(_py, CanonicalSpecialSingleton::NotImplemented)
}

pub(crate) fn is_not_implemented_bits(_py: &PyToken<'_>, bits: u64) -> bool {
    if let Some(ptr) = maybe_ptr_from_bits(bits) {
        unsafe { object_type_id(ptr) == TYPE_ID_NOT_IMPLEMENTED }
    } else {
        false
    }
}

pub(crate) fn ellipsis_bits(_py: &PyToken<'_>) -> u64 {
    canonical_special_singleton_bits(_py, CanonicalSpecialSingleton::Ellipsis)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_immortal_singleton(_py: &PyToken<'_>, bits: u64, type_id: u32) {
        let ptr = obj_from_bits(bits)
            .as_ptr()
            .expect("special singleton must be heap allocated");
        unsafe {
            let header = header_from_obj_ptr(ptr);
            assert_eq!(object_type_id(ptr), type_id);
            assert_ne!(
                (*header).load_metadata_flags() & crate::object::HEADER_FLAG_IMMORTAL,
                0
            );
            let refcount = (*header).ref_count_snapshot();
            dec_ref_bits(_py, bits);
            dec_ref_bits(_py, bits);
            assert_eq!(object_type_id(ptr), type_id);
            assert_eq!((*header).ref_count_snapshot(), refcount);
        }
    }

    #[test]
    fn special_singletons_are_immortal_process_roots() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            assert_immortal_singleton(_py, missing_bits(_py), TYPE_ID_OBJECT);
            assert_immortal_singleton(_py, not_implemented_bits(_py), TYPE_ID_NOT_IMPLEMENTED);
            assert_immortal_singleton(_py, ellipsis_bits(_py), TYPE_ID_ELLIPSIS);
        });
    }
}
