//! Canonical itertools class construction shared by reduced and full profiles.

use crate::*;
use molt_runtime_core::ObjectShapeId;

/// Allocate and publish one itertools class with its generated object shape.
///
/// The shape is installed before instances can be allocated, so lifecycle
/// dispatch never observes a generic object shape for an itertools payload.
pub(crate) fn alloc_itertools_class(
    _py: &crate::PyToken<'_>,
    name: &str,
    layout_size: i64,
    shape: ObjectShapeId,
) -> u64 {
    let name_str_ptr = alloc_string(_py, name.as_bytes());
    if name_str_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let name_bits = MoltObject::from_ptr(name_str_ptr).bits();
    let class_ptr = alloc_class_obj(_py, name_bits);
    dec_ref_bits(_py, name_bits);
    if class_ptr.is_null() {
        return MoltObject::none().bits();
    }
    let class_bits = MoltObject::from_ptr(class_ptr).bits();
    unsafe {
        crate::object::class_storage::class_declare_native_slots(
            class_ptr,
            crate::object::class_storage::ClassSlotPolicy {
                allows_dict: false,
                allows_weakref: shape == ObjectShapeId::ItertoolsTee,
                variable_sized: false,
            },
        );
    }
    if !unsafe { crate::object::class_set_instance_shape_id(class_ptr, shape) } {
        dec_ref_bits(_py, class_bits);
        return MoltObject::none().bits();
    }
    let builtins = builtin_classes(_py);
    unsafe {
        if let Some(ptr) = obj_from_bits(class_bits).as_ptr()
            && !crate::object::object_init_class_edge_unpublished(
                _py,
                ptr,
                builtins.type_obj,
                ClassEdgeOwnership::Owned,
            )
        {
            dec_ref_bits(_py, class_bits);
            return MoltObject::none().bits();
        }
    }
    let _ = molt_class_set_base(class_bits, builtins.object);
    let dict_bits = unsafe { class_dict_bits(class_ptr) };
    if let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr()
        && unsafe { object_type_id(dict_ptr) } == TYPE_ID_DICT
    {
        let layout_name = intern_static_name(
            _py,
            &crate::runtime_state(_py).interned.molt_layout_size,
            b"__molt_layout_size__",
        );
        let layout_bits = MoltObject::from_int(layout_size).bits();
        unsafe { dict_set_in_place(_py, dict_ptr, layout_name, layout_bits) };
    }
    if exception_pending(_py)
        || !unsafe {
            crate::builtins::attr::capture_class_slot_declaration_for_seal(_py, class_ptr)
        }
    {
        dec_ref_bits(_py, class_bits);
        return MoltObject::none().bits();
    }
    class_bits
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cached_public_results_own_references_independent_of_runtime_slots() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            // Both profiles call these same exports: reduced builds include the
            // source in-tree, while edge/full builds use the satellite bridge.
            for (get, minimum_owners) in [
                (crate::molt_itertools_kwd_mark as extern "C" fn() -> u64, 2),
                (
                    crate::molt_itertools_repeat_type as extern "C" fn() -> u64,
                    3,
                ),
            ] {
                let first = get();
                assert!(!exception_pending(py));
                let ptr = obj_from_bits(first).as_ptr().expect("cached heap result");
                let owners = unsafe { (*header_from_obj_ptr(ptr)).owned_ref_count_snapshot() };
                // Cache + caller; the repeat class also has its MRO self-edge.
                assert!(
                    owners >= minimum_owners,
                    "public result consumed a cache owner"
                );
                for _ in 0..4 {
                    let next = get();
                    assert_eq!(first, next);
                    assert_eq!(
                        unsafe { (*header_from_obj_ptr(ptr)).owned_ref_count_snapshot() },
                        owners + 1,
                        "every public return must supply a fresh reference"
                    );
                    dec_ref_bits(py, next);
                    assert_eq!(
                        unsafe { (*header_from_obj_ptr(ptr)).owned_ref_count_snapshot() },
                        owners
                    );
                }
                dec_ref_bits(py, first);
            }
        });
    }

    #[test]
    fn class_construction_publishes_generated_instance_shape() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let shape = ObjectShapeId::ItertoolsCount;
            let class_bits = alloc_itertools_class(py, "count", 24, shape);
            let class_ptr = obj_from_bits(class_bits)
                .as_ptr()
                .expect("itertools class allocation must succeed");
            assert_eq!(
                unsafe { crate::object::class_instance_shape_id(class_ptr) },
                shape
            );
            dec_ref_bits(py, class_bits);
        });
    }
}
