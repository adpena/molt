//! Structural ancestry reads borrow class-owned immutable MROs under the GIL.

use super::*;
use crate::builtins::exceptions::{exception_last_bits_noinc, record_exception};
use molt_cpython_abi::bridge::GLOBAL_BRIDGE;

fn named_class(py: &PyToken<'_>, name: &[u8]) -> u64 {
    let name = alloc_string(py, name);
    assert!(!name.is_null());
    let name = MoltObject::from_ptr(name).bits();
    let class = crate::molt_class_new_attached_body(py, name);
    dec_ref_bits(py, name);
    assert!(obj_from_bits(class).as_ptr().is_some());
    class
}

#[test]
fn managed_ancestry_preserves_identity_before_and_after_mro_publication() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            // Equal spellings deliberately describe three distinct classes.
            let base = named_class(py, b"StructuralIdentity");
            let child = named_class(py, b"StructuralIdentity");
            let unrelated = named_class(py, b"StructuralIdentity");
            let builtins = builtin_classes(py);
            crate::molt_class_set_base_attached_body(py, base, builtins.object);
            crate::molt_class_set_base_attached_body(py, unrelated, builtins.object);
            assert!(!exception_pending(py));
            let child_ptr = obj_from_bits(child).as_ptr().unwrap();
            crate::object::layout::class_set_bases_bits(py, child_ptr, base);
            assert!(obj_from_bits(class_mro_bits(child_ptr)).is_none());
            let pending = crate::builtins::exceptions::alloc_exception(
                py,
                BuiltinExc::ValueError,
                "preserve structural-query error",
            );
            assert!(!pending.is_null());
            let pending_bits = MoltObject::from_ptr(pending).bits();
            for published in [false, true] {
                if published {
                    crate::molt_class_set_base_attached_body(py, child, base);
                    assert!(!exception_pending(py));
                    let mro = obj_from_bits(class_mro_bits(child_ptr)).as_ptr().unwrap();
                    assert_eq!(object_type_id(mro), TYPE_ID_TUPLE);
                }
                record_exception(py, pending);
                // The expected relation follows the declared graph, not an MRO
                // reconstructed by the implementation under test.
                for (derived, parent, expected) in [
                    (child, child, true),
                    (child, base, true),
                    (child, builtins.object, true),
                    (base, child, false),
                    (child, unrelated, false),
                    (unrelated, child, false),
                    (child, builtins.int, false),
                    (MoltObject::from_int(1).bits(), child, false),
                ] {
                    assert_eq!(is_real_subtype(py, derived, parent), expected);
                    assert!(exception_pending(py));
                    assert_eq!(exception_last_bits_noinc(py), Some(pending_bits));
                }
                clear_exception(py);
            }
            dec_ref_bits(py, pending_bits);
            dec_ref_bits(py, child);
            dec_ref_bits(py, unrelated);
            dec_ref_bits(py, base);
        }
    });
}

#[test]
fn managed_ancestry_keeps_published_native_mro_ownership_and_pending_identity() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    assert!(crate::cpython_abi_hooks::register_cpython_hooks());
    crate::with_gil_entry_nopanic!(py, {
        unsafe {
            let base = named_class(py, b"PublishedStructuralBase");
            let child = named_class(py, b"PublishedStructuralChild");
            let builtins = builtin_classes(py);
            crate::molt_class_set_base_attached_body(py, base, builtins.object);
            crate::object::class_finish_definition(py, obj_from_bits(base).as_ptr().unwrap())
                .expect("completed base class");
            crate::molt_class_set_base_attached_body(py, child, base);
            assert!(!exception_pending(py));
            let child_ptr = obj_from_bits(child).as_ptr().unwrap();
            crate::object::class_finish_definition(py, child_ptr).expect("completed child class");
            let mro_bits = class_mro_bits(child_ptr);
            let mro = obj_from_bits(mro_bits).as_ptr().unwrap();
            let native_mro = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(mro_bits);
            assert!(!native_mro.is_null());
            assert!(!exception_pending(py));
            let managed_refs = (*header_from_obj_ptr(mro)).ref_count_snapshot();
            let native_refs = (*native_mro).ob_refcnt;
            let pending = crate::builtins::exceptions::alloc_exception(
                py,
                BuiltinExc::ValueError,
                "preserve published-query error",
            );
            assert!(!pending.is_null());
            let pending_bits = MoltObject::from_ptr(pending).bits();
            record_exception(py, pending);
            for _ in 0..32 {
                assert!(is_real_subtype(py, child, base));
                assert!(is_real_subtype(py, child, builtins.object));
                assert!(is_real_subtype(py, child, child));
                assert!(!is_real_subtype(py, base, child));
                assert!(!is_real_subtype(py, child, builtins.int));
                assert_eq!(exception_last_bits_noinc(py), Some(pending_bits));
                assert_eq!(
                    (*header_from_obj_ptr(mro)).ref_count_snapshot(),
                    managed_refs
                );
                assert_eq!((*native_mro).ob_refcnt, native_refs);
            }
            // Final counts establish lifetime balance, not the absence of a
            // transient pin: emitted-code and performance evidence own that.
            clear_exception(py);
            dec_ref_bits(py, pending_bits);
            dec_ref_bits(py, child);
            dec_ref_bits(py, base);
        }
    });
}
