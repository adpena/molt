use crate::PyToken;
#[cfg(test)]
mod foreign_descriptor_tests;
use crate::builtins::functions::native_callable::NativeCallableSpec;
use crate::object::ops_format::string_obj_bytes;
use std::cell::RefCell;

use molt_obj_model::{ExceptionTypedField, MoltObject};

use crate::builtins::annotations::pep649_enabled;
use crate::builtins::exceptions::{
    exception_matches_builtin_name, exception_typed_fields_replace_internal,
    molt_exception_last_pending,
};
use crate::{
    ClassEdgeOwnership, FIELD_OFFSET_IC_HIT_COUNT, FIELD_OFFSET_IC_MISS_COUNT, TYPE_ID_CALL_ITER,
    TYPE_ID_CLASSMETHOD, TYPE_ID_DATACLASS, TYPE_ID_DICT, TYPE_ID_ENUMERATE, TYPE_ID_FILE_HANDLE,
    TYPE_ID_FILTER, TYPE_ID_FUNCTION, TYPE_ID_GENERATOR, TYPE_ID_GLOB_ITER, TYPE_ID_ITER,
    TYPE_ID_MAP, TYPE_ID_NATIVE_DESCRIPTOR, TYPE_ID_PROPERTY, TYPE_ID_REVERSED,
    TYPE_ID_STATICMETHOD, TYPE_ID_STRING, TYPE_ID_TYPE, TYPE_ID_ZIP, alloc_dict_with_pairs,
    alloc_function_obj, alloc_property_obj, alloc_string, alloc_tuple, builtin_class_method_bits,
    builtin_classes, builtin_func_bits, call_callable1, call_callable2, call_callable3,
    class_bases_bits, class_bases_vec, class_dict_bits, class_layout_version_bits,
    class_mro_pinned, class_mro_view, class_name_bits, clear_exception, dataclass_desc_ptr,
    dec_ref_bits, dict_get_in_place, dict_set_in_place, exception_last_bits_noinc,
    exception_pending, inc_ref_bits, init_atomic_bits, intern_static_name, is_builtin_class_bits,
    is_truthy, maybe_ptr_from_bits, module_dict_bits, molt_awaitable_await, molt_bound_method_new,
    molt_function_get_code, molt_function_get_globals, obj_from_bits, object_class_bits,
    object_type_id, profile_hit_unchecked, raise_exception, runtime_state, string_bytes,
    string_len, string_obj_to_owned, type_name, type_of_bits,
};

const ATTR_NAME_INLINE_CAP: usize = 32;

enum AttrNameCacheKey {
    Inline {
        len: u8,
        bytes: [u8; ATTR_NAME_INLINE_CAP],
    },
    Heap(Vec<u8>),
}

impl AttrNameCacheKey {
    fn new(bytes: &[u8]) -> Self {
        if bytes.len() <= ATTR_NAME_INLINE_CAP {
            let mut inline = [0u8; ATTR_NAME_INLINE_CAP];
            inline[..bytes.len()].copy_from_slice(bytes);
            Self::Inline {
                len: bytes.len() as u8,
                bytes: inline,
            }
        } else {
            Self::Heap(bytes.to_vec())
        }
    }

    fn as_slice(&self) -> &[u8] {
        match self {
            Self::Inline { len, bytes } => &bytes[..usize::from(*len)],
            Self::Heap(bytes) => bytes.as_slice(),
        }
    }
}

#[cfg(test)]
mod descriptor_tests;

#[cfg(test)]
mod tests {
    use super::{
        ATTR_NAME_INLINE_CAP, AttrNameCacheKey, clear_attr_tls_caches, descriptor_cache_lookup,
        descriptor_cache_store, set_attribute_error_members,
    };
    use crate::{MoltObject, alloc_string, dec_ref_bits, obj_from_bits};

    fn heap_refcount(bits: u64) -> u32 {
        let ptr = obj_from_bits(bits).as_ptr().expect("expected heap bits");
        unsafe { (*crate::object::header_from_obj_ptr(ptr)).ref_count_snapshot() }
    }

    #[test]
    fn physical_slot_declaration_survives_public_rebind_mutation_and_iterator_exhaustion() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            for kind in 0..4 {
                let name = crate::attr_name_bits_from_bytes(_py, b"SealedSlots").unwrap();
                let field = crate::attr_name_bits_from_bytes(_py, b"field").unwrap();
                let dict = crate::attr_name_bits_from_bytes(_py, b"__dict__").unwrap();
                let weakref = crate::attr_name_bits_from_bytes(_py, b"__weakref__").unwrap();
                let slots = crate::attr_name_bits_from_bytes(_py, b"__slots__").unwrap();
                let class = crate::molt_class_new(name);
                let ptr = obj_from_bits(class).as_ptr().unwrap();
                let declaration = match kind {
                    0 => {
                        crate::inc_ref_bits(_py, field);
                        field
                    }
                    1 => MoltObject::from_ptr(crate::alloc_tuple(_py, &[field, dict, weakref]))
                        .bits(),
                    2 => {
                        MoltObject::from_ptr(crate::alloc_list(_py, &[field, dict, weakref])).bits()
                    }
                    _ => {
                        let tuple =
                            MoltObject::from_ptr(crate::alloc_tuple(_py, &[field, dict, weakref]))
                                .bits();
                        let iterator = crate::molt_iter(tuple);
                        dec_ref_bits(_py, tuple);
                        iterator
                    }
                };
                crate::molt_set_attr_name(class, slots, declaration);
                unsafe {
                    crate::object::class_finish_definition(_py, ptr).expect("seal valid slots");
                    let offset = super::class_own_slot_field_offset(_py, ptr, field).unwrap();
                    let captured = crate::object::layout::class_slot_declaration_bits(ptr);
                    assert_ne!(captured, declaration, "the layout tuple must be private");
                    assert!(
                        crate::object::object_payload_size(ptr)
                            >= crate::object::layout::CLASS_PAYLOAD_WORDS
                                * std::mem::size_of::<u64>()
                    );
                    let mut edges = Vec::new();
                    crate::object::heap_lifecycle::visit_owned_values(_py, ptr, &mut |edge| {
                        edges.push(edge)
                    });
                    assert!(
                        edges.contains(&captured),
                        "declaration is a traced class owner"
                    );
                    // A second layout pass must not consume an iterator again.
                    crate::object::class_finish_definition(_py, ptr).expect("seal valid slots");
                    crate::object::class_finish_definition(_py, ptr).expect("seal valid slots");
                    if kind == 2 {
                        crate::molt_list_clear(declaration);
                    }
                    let empty = MoltObject::from_ptr(crate::alloc_tuple(_py, &[])).bits();
                    crate::molt_set_attr_name(class, slots, empty);
                    dec_ref_bits(_py, empty);
                    assert_eq!(
                        crate::object::layout::class_slot_declaration_bits(ptr),
                        captured
                    );
                    assert_eq!(
                        super::class_own_slot_field_offset(_py, ptr, field),
                        Some(offset)
                    );
                    let info = super::class_slots_info(_py, ptr).unwrap();
                    assert_eq!(info.allows_dict, kind != 0);
                    assert_eq!(info.allows_weakref, kind != 0);
                }
                assert!(!crate::exception_pending(_py));
                for bits in [name, field, dict, weakref, slots, class, declaration] {
                    dec_ref_bits(_py, bits);
                }
            }
        });
    }

    #[test]
    fn absence_of_slots_is_sealed_and_invalid_declarations_do_not_publish_provenance() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let name = crate::attr_name_bits_from_bytes(_py, b"Unslotted").unwrap();
            let slots = crate::attr_name_bits_from_bytes(_py, b"__slots__").unwrap();
            let empty = MoltObject::from_ptr(crate::alloc_tuple(_py, &[])).bits();
            let class = crate::molt_class_new(name);
            let ptr = obj_from_bits(class).as_ptr().unwrap();
            unsafe {
                crate::object::class_finish_definition(_py, ptr).expect("seal unslotted class");
            }
            crate::molt_set_attr_name(class, slots, empty);
            let policy = unsafe { super::class_slots_info(_py, ptr) }.unwrap();
            assert!(policy.allows_dict && policy.allows_weakref);
            assert!(matches!(
                unsafe { crate::object::layout::class_slot_declaration(ptr) },
                crate::object::layout::ClassSlotDeclaration::Absent
            ));

            let invalid_class = crate::molt_class_new(name);
            let invalid_ptr = obj_from_bits(invalid_class).as_ptr().unwrap();
            let invalid =
                MoltObject::from_ptr(crate::alloc_tuple(_py, &[MoltObject::from_int(1).bits()]))
                    .bits();
            crate::molt_set_attr_name(invalid_class, slots, invalid);
            assert!(unsafe { crate::object::class_finish_definition(_py, invalid_ptr) }.is_err());
            assert!(crate::exception_pending(_py));
            assert!(!unsafe { crate::object::class_definition_is_finished(invalid_ptr) });
            assert!(
                unsafe { crate::object::layout::class_cached_layout_size(invalid_ptr) }.is_none()
            );
            crate::molt_exception_clear();
            assert!(matches!(
                unsafe { crate::object::layout::class_slot_declaration(invalid_ptr) },
                crate::object::layout::ClassSlotDeclaration::Uninitialized
            ));
            crate::molt_set_attr_name(invalid_class, slots, empty);
            unsafe { crate::object::class_finish_definition(_py, invalid_ptr) }
                .expect("retry valid declaration");
            assert!(
                !unsafe { super::class_slots_info(_py, invalid_ptr) }
                    .unwrap()
                    .allows_dict
            );
            assert!(!crate::exception_pending(_py));
            for bits in [name, slots, empty, class, invalid_class, invalid] {
                dec_ref_bits(_py, bits);
            }
        });
    }

    #[test]
    fn sealed_field_offsets_retain_exact_map_and_ignore_namespace_rebinding() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let name = crate::attr_name_bits_from_bytes(_py, b"SealedFieldOffsets").unwrap();
            let field = crate::attr_name_bits_from_bytes(_py, b"field").unwrap();
            let offsets_name =
                crate::attr_name_bits_from_bytes(_py, b"__molt_field_offsets__").unwrap();
            let size_name = crate::attr_name_bits_from_bytes(_py, b"__molt_layout_size__").unwrap();
            let class = crate::molt_class_new(name);
            let class_ptr = obj_from_bits(class).as_ptr().unwrap();
            let offsets_ptr =
                crate::alloc_dict_with_pairs(_py, &[field, MoltObject::from_int(0).bits()]);
            let offsets = MoltObject::from_ptr(offsets_ptr).bits();
            crate::molt_set_attr_name(class, offsets_name, offsets);
            crate::molt_set_attr_name(class, size_name, MoltObject::from_int(16).bits());
            unsafe { crate::object::class_finish_definition(_py, class_ptr) }
                .expect("seal exact field map");
            assert_eq!(
                unsafe { crate::object::layout::class_field_offsets_bits(class_ptr) },
                offsets,
            );
            let namespace = obj_from_bits(unsafe { crate::class_dict_bits(class_ptr) })
                .as_ptr()
                .unwrap();
            assert!(unsafe { crate::dict_get_in_place(_py, namespace, offsets_name) }.is_none());
            assert!(unsafe { crate::dict_get_in_place(_py, namespace, size_name) }.is_none());

            let rebound_ptr =
                crate::alloc_dict_with_pairs(_py, &[field, MoltObject::from_int(8).bits()]);
            let rebound = MoltObject::from_ptr(rebound_ptr).bits();
            crate::molt_set_attr_name(class, offsets_name, rebound);
            assert!(crate::exception_pending(_py));
            let exception = crate::molt_exception_last();
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                _py,
                exception,
                "TypeError",
            ));
            crate::molt_exception_clear();
            dec_ref_bits(_py, exception);
            assert_eq!(
                unsafe { super::class_field_offset(_py, class_ptr, field) },
                Some(0)
            );
            crate::molt_del_attr_name(class, offsets_name);
            assert!(crate::exception_pending(_py));
            let exception = crate::molt_exception_last();
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                _py,
                exception,
                "TypeError",
            ));
            crate::molt_exception_clear();
            dec_ref_bits(_py, exception);
            assert_eq!(
                unsafe { super::class_field_offset(_py, class_ptr, field) },
                Some(0)
            );

            let namespace = obj_from_bits(unsafe { crate::class_dict_bits(class_ptr) })
                .as_ptr()
                .unwrap();
            unsafe { crate::dict_set_in_place(_py, namespace, offsets_name, rebound) };
            assert_eq!(
                unsafe { super::class_field_offset(_py, class_ptr, field) },
                Some(0)
            );
            assert!(unsafe { crate::dict_del_in_place(_py, namespace, offsets_name) });
            assert_eq!(
                unsafe { super::class_field_offset(_py, class_ptr, field) },
                Some(0)
            );
            assert_eq!(
                unsafe { crate::object::layout::class_field_offsets_bits(class_ptr) },
                offsets,
            );

            let mut edges = Vec::new();
            unsafe {
                crate::object::heap_lifecycle::visit_owned_values(_py, class_ptr, &mut |bits| {
                    edges.push(bits)
                });
            }
            let record = unsafe { crate::object::layout::class_field_layout_bits(class_ptr) };
            assert_eq!(edges.iter().filter(|&&bits| bits == record).count(), 1);
            assert!(
                !edges.contains(&offsets),
                "the record owns the map projection"
            );
            let mut record_edges = Vec::new();
            unsafe {
                crate::object::heap_lifecycle::visit_owned_values(
                    _py,
                    obj_from_bits(record).as_ptr().unwrap(),
                    &mut |bits| record_edges.push(bits),
                );
            }
            assert_eq!(
                record_edges.iter().filter(|&&bits| bits == offsets).count(),
                1
            );
            assert_eq!(heap_refcount(offsets), 2, "local plus private record owner");
            for _ in 0..2 {
                unsafe { crate::object::heap_lifecycle::clear_cycle_edges(_py, class_ptr) };
                assert_eq!(
                    unsafe { crate::object::layout::class_field_layout_bits(class_ptr) },
                    record,
                    "cycle clear must retain the physical layout identity"
                );
                assert_eq!(
                    unsafe { crate::object::layout::class_field_offsets_bits(class_ptr) },
                    offsets,
                    "live instances still require the exact physical field map"
                );
                assert_eq!(
                    heap_refcount(offsets),
                    2,
                    "cycle clear must preserve the local and private-record owners"
                );
            }
            dec_ref_bits(_py, class);
            assert_eq!(
                heap_refcount(offsets),
                1,
                "terminal class release must discharge the private map edge exactly once"
            );

            for bits in [name, field, offsets_name, size_name, offsets, rebound] {
                dec_ref_bits(_py, bits);
            }
            assert!(!crate::exception_pending(_py));
        });
    }

    #[test]
    fn malformed_field_offsets_fail_before_private_publication() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let name = crate::attr_name_bits_from_bytes(_py, b"InvalidFieldOffsets").unwrap();
            let field = crate::attr_name_bits_from_bytes(_py, b"field").unwrap();
            let other = crate::attr_name_bits_from_bytes(_py, b"other").unwrap();
            let offsets_name =
                crate::attr_name_bits_from_bytes(_py, b"__molt_field_offsets__").unwrap();
            let size_name = crate::attr_name_bits_from_bytes(_py, b"__molt_layout_size__").unwrap();
            let cases = [
                (
                    MoltObject::from_int(1).bits(),
                    MoltObject::from_int(0).bits(),
                    16,
                ),
                (field, MoltObject::from_bool(false).bits(), 16),
                (field, MoltObject::from_int(-8).bits(), 16),
                (field, MoltObject::from_int(4).bits(), 24),
                (field, MoltObject::from_int(8).bits(), 16),
            ];
            for (key, offset, size) in cases {
                let class = crate::molt_class_new(name);
                let class_ptr = obj_from_bits(class).as_ptr().unwrap();
                let offsets_ptr = crate::alloc_dict_with_pairs(_py, &[key, offset]);
                let offsets = MoltObject::from_ptr(offsets_ptr).bits();
                crate::molt_set_attr_name(class, offsets_name, offsets);
                crate::molt_set_attr_name(class, size_name, MoltObject::from_int(size).bits());
                assert!(unsafe { crate::object::class_finish_definition(_py, class_ptr) }.is_err());
                assert!(crate::exception_pending(_py));
                assert_eq!(
                    unsafe { crate::object::layout::class_field_offsets_bits(class_ptr) },
                    0,
                );
                assert!(
                    unsafe { crate::object::layout::class_cached_layout_size(class_ptr) }.is_none()
                );
                assert!(!unsafe { crate::object::class_definition_is_finished(class_ptr) });
                crate::molt_exception_clear();
                dec_ref_bits(_py, offsets);
                dec_ref_bits(_py, class);
            }

            let class = crate::molt_class_new(name);
            let class_ptr = obj_from_bits(class).as_ptr().unwrap();
            let offsets_ptr = crate::alloc_dict_with_pairs(
                _py,
                &[
                    field,
                    MoltObject::from_int(0).bits(),
                    other,
                    MoltObject::from_int(0).bits(),
                ],
            );
            let offsets = MoltObject::from_ptr(offsets_ptr).bits();
            crate::molt_set_attr_name(class, offsets_name, offsets);
            crate::molt_set_attr_name(class, size_name, MoltObject::from_int(16).bits());
            assert!(unsafe { crate::object::class_finish_definition(_py, class_ptr) }.is_err());
            assert_eq!(
                unsafe { crate::object::layout::class_field_offsets_bits(class_ptr) },
                0,
            );
            crate::molt_exception_clear();
            dec_ref_bits(_py, offsets);
            dec_ref_bits(_py, class);

            for bits in [name, field, other, offsets_name, size_name] {
                dec_ref_bits(_py, bits);
            }
            assert!(!crate::exception_pending(_py));
        });
    }

    #[test]
    fn attr_name_cache_key_inlines_common_attr_names() {
        let key = AttrNameCacheKey::new(b"__molt_arg_names__");

        assert!(matches!(key, AttrNameCacheKey::Inline { .. }));
        assert_eq!(key.as_slice(), b"__molt_arg_names__");
    }

    #[test]
    fn attr_name_cache_key_preserves_long_names() {
        let bytes = vec![b'x'; ATTR_NAME_INLINE_CAP + 1];
        let key = AttrNameCacheKey::new(&bytes);

        assert!(matches!(key, AttrNameCacheKey::Heap(_)));
        assert_eq!(key.as_slice(), bytes.as_slice());
    }

    #[test]
    fn attribute_error_members_use_canonical_typed_fields_without_overwriting_args() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let obj_ptr = alloc_string(_py, b"attribute-owner");
            assert!(!obj_ptr.is_null());
            let obj_bits = MoltObject::from_ptr(obj_ptr).bits();
            let obj_refcount = heap_refcount(obj_bits);
            let exc_ptr = crate::builtins::exceptions::alloc_exception(
                _py,
                "AttributeError",
                "missing attribute",
            );
            assert!(!exc_ptr.is_null());
            let exc_bits = MoltObject::from_ptr(exc_ptr).bits();
            let args_bits = unsafe { crate::builtins::exceptions::exception_args_bits(exc_ptr) };
            let args_refcount = heap_refcount(args_bits);

            set_attribute_error_members(_py, exc_bits, "missing", obj_bits)
                .expect("publish AttributeError typed members");

            assert_eq!(
                unsafe { crate::builtins::exceptions::exception_args_bits(exc_ptr) },
                args_bits,
                "AttributeError member publication must not overwrite args"
            );
            assert_eq!(heap_refcount(args_bits), args_refcount);
            let name_bits = crate::builtins::exceptions::exception_typed_field_get(
                _py,
                exc_ptr,
                molt_obj_model::ExceptionTypedField::AttributeErrorName,
            )
            .expect("AttributeError.name descriptor")
            .expect("AttributeError.name value");
            let member_obj_bits = crate::builtins::exceptions::exception_typed_field_get(
                _py,
                exc_ptr,
                molt_obj_model::ExceptionTypedField::AttributeErrorObject,
            )
            .expect("AttributeError.obj descriptor")
            .expect("AttributeError.obj value");
            assert_eq!(
                crate::string_obj_to_owned(obj_from_bits(name_bits)).as_deref(),
                Some("missing")
            );
            assert_eq!(member_obj_bits, obj_bits);
            assert_eq!(heap_refcount(obj_bits), obj_refcount + 2);
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, member_obj_bits);
            assert_eq!(heap_refcount(obj_bits), obj_refcount + 1);

            dec_ref_bits(_py, exc_bits);
            assert_eq!(heap_refcount(obj_bits), obj_refcount);
            dec_ref_bits(_py, obj_bits);
        });
    }

    #[test]
    fn descriptor_cache_store_owns_released_heap_bits() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            clear_attr_tls_caches(_py);

            let class_ptr = alloc_string(_py, b"descriptor-cache-class-owner");
            assert!(!class_ptr.is_null());
            let attr_ptr = alloc_string(_py, b"cached_attr");
            assert!(!attr_ptr.is_null());
            let first_ptr = alloc_string(_py, b"first-cached-value");
            assert!(!first_ptr.is_null());
            let second_ptr = alloc_string(_py, b"second-cached-value");
            assert!(!second_ptr.is_null());

            let class_bits = MoltObject::from_ptr(class_ptr).bits();
            let attr_bits = MoltObject::from_ptr(attr_ptr).bits();
            let first_bits = MoltObject::from_ptr(first_ptr).bits();
            let second_bits = MoltObject::from_ptr(second_ptr).bits();

            let class_before = heap_refcount(class_bits);
            let first_before = heap_refcount(first_bits);
            let second_before = heap_refcount(second_bits);

            descriptor_cache_store(_py, class_bits, attr_bits, 1, None, Some(first_bits));
            assert_eq!(heap_refcount(class_bits), class_before + 1);
            assert_eq!(heap_refcount(first_bits), first_before + 1);
            assert_eq!(heap_refcount(second_bits), second_before);
            let cached = descriptor_cache_lookup(_py, class_bits, attr_bits, 1)
                .expect("descriptor cache should contain first value");
            assert_eq!(cached.class_attr_bits, Some(first_bits));
            assert_eq!(heap_refcount(class_bits), class_before + 2);
            assert_eq!(heap_refcount(first_bits), first_before + 2);
            // Reentrant lookup may evict the TLS entry while this operation
            // still needs its original selected value and class owner.
            descriptor_cache_store(_py, class_bits, attr_bits, 2, None, Some(second_bits));
            assert_eq!(cached.class_attr_bits, Some(first_bits));
            assert_eq!(heap_refcount(class_bits), class_before + 2);
            assert_eq!(heap_refcount(first_bits), first_before + 1);
            assert_eq!(heap_refcount(second_bits), second_before + 1);
            cached.release(_py);
            assert_eq!(heap_refcount(class_bits), class_before + 1);
            assert_eq!(heap_refcount(first_bits), first_before);
            let cached = descriptor_cache_lookup(_py, class_bits, attr_bits, 2)
                .expect("descriptor cache should contain replacement value");
            assert_eq!(cached.class_attr_bits, Some(second_bits));
            assert_eq!(heap_refcount(class_bits), class_before + 2);
            assert_eq!(heap_refcount(second_bits), second_before + 2);
            cached.release(_py);
            assert_eq!(heap_refcount(class_bits), class_before + 1);
            assert_eq!(heap_refcount(second_bits), second_before + 1);

            clear_attr_tls_caches(_py);
            assert_eq!(heap_refcount(class_bits), class_before);
            assert_eq!(heap_refcount(first_bits), first_before);
            assert_eq!(heap_refcount(second_bits), second_before);

            dec_ref_bits(_py, second_bits);
            dec_ref_bits(_py, first_bits);
            dec_ref_bits(_py, attr_bits);
            dec_ref_bits(_py, class_bits);
        });
    }
}

struct AttrNameCacheEntry {
    key: AttrNameCacheKey,
    bits: u64,
}

/// Direct-mapped cache with 16 slots for attribute name -> string bits.
/// Keyed by a simple hash of the byte slice.  This replaces the previous
/// single-entry cache that thrashed on every alternating attribute name
/// (e.g. `__iter__` / `__next__` in a for-loop body caused 2M+ allocs in
/// bench_sum_list).
const ATTR_NAME_CACHE_SIZE: usize = 16; // must be power of 2

struct AttrNameCache {
    slots: [Option<AttrNameCacheEntry>; ATTR_NAME_CACHE_SIZE],
}

impl AttrNameCache {
    const fn new() -> Self {
        // Work around const-init limitations: build the array element-by-element.
        const NONE: Option<AttrNameCacheEntry> = None;
        Self {
            slots: [NONE; ATTR_NAME_CACHE_SIZE],
        }
    }

    #[inline]
    fn slot_index(bytes: &[u8]) -> usize {
        // FNV-1a-inspired fast hash – only needs to spread common dunder
        // names across 16 buckets.
        let mut h: u32 = 0x811c_9dc5;
        for &b in bytes {
            h ^= b as u32;
            h = h.wrapping_mul(0x0100_0193);
        }
        (h as usize) & (ATTR_NAME_CACHE_SIZE - 1)
    }

    fn lookup(&self, bytes: &[u8]) -> Option<u64> {
        let idx = Self::slot_index(bytes);
        self.slots[idx]
            .as_ref()
            .filter(|e| e.key.as_slice() == bytes)
            .map(|e| e.bits)
    }

    fn insert(&mut self, _py: &PyToken<'_>, bytes: &[u8], bits: u64) -> Option<u64> {
        let idx = Self::slot_index(bytes);
        let previous = self.slots[idx].take().map(|entry| entry.bits);
        inc_ref_bits(_py, bits);
        self.slots[idx] = Some(AttrNameCacheEntry {
            key: AttrNameCacheKey::new(bytes),
            bits,
        });
        previous
    }

    fn take_entries(&mut self) -> [Option<AttrNameCacheEntry>; ATTR_NAME_CACHE_SIZE] {
        std::mem::replace(&mut self.slots, AttrNameCache::new().slots)
    }
}

/// Cache identity stays in TLS; a lookup retains only the selected owners.
struct DescriptorCacheEntry {
    attr_name: Vec<u8>,
    version: u64,
    snapshot: DescriptorSnapshot,
}

impl DescriptorCacheEntry {
    fn release(self, py: &PyToken<'_>) {
        self.snapshot.release(py);
    }
}

/// Pins the initial class lookup across callbacks and cache eviction. It has
/// no lookup key: consumers must not revalidate or repeat the MRO lookup after
/// an observable callback changes the class dictionary.
pub(crate) struct DescriptorSnapshot {
    pub(crate) class_bits: u64,
    pub(crate) data_desc_bits: Option<u64>,
    pub(crate) class_attr_bits: Option<u64>,
}

impl DescriptorSnapshot {
    fn retained(&self, py: &PyToken<'_>) -> Self {
        Self::retain(
            py,
            self.class_bits,
            self.data_desc_bits,
            self.class_attr_bits,
        )
    }

    fn retain(
        _py: &PyToken<'_>,
        class_bits: u64,
        data_desc_bits: Option<u64>,
        class_attr_bits: Option<u64>,
    ) -> Self {
        if class_bits != 0 {
            inc_ref_bits(_py, class_bits);
        }
        if let Some(bits) = data_desc_bits
            && bits != 0
        {
            inc_ref_bits(_py, bits);
        }
        if let Some(bits) = class_attr_bits
            && bits != 0
        {
            inc_ref_bits(_py, bits);
        }
        Self {
            class_bits,
            data_desc_bits,
            class_attr_bits,
        }
    }

    pub(crate) fn release(self, _py: &PyToken<'_>) {
        if self.class_bits != 0 {
            dec_ref_bits(_py, self.class_bits);
        }
        if let Some(bits) = self.data_desc_bits
            && bits != 0
        {
            dec_ref_bits(_py, bits);
        }
        if let Some(bits) = self.class_attr_bits
            && bits != 0
        {
            dec_ref_bits(_py, bits);
        }
    }
}

// ---------------------------------------------------------------------------
// Field-offset inline cache (IC) — CPython 3.12 LOAD_ATTR_INSTANCE_VALUE
// ---------------------------------------------------------------------------
// Direct-mapped, 32-slot TLS cache keyed by (class_bits, attr_name hash).
// On hit we skip the sealed inferred-field lookup and go
// straight to a single `object_field_get_ptr_raw` call.  Invalidated via the
// global type version counter (bumped when any class __dict__ is modified).

const FIELD_OFFSET_IC_SIZE: usize = 32; // must be power of 2

#[derive(Clone, Copy)]
struct FieldOffsetICEntry {
    /// NaN-boxed class bits (identity of the type object)
    class_bits: u64,
    /// Hash of the attribute name bytes (for fast comparison)
    name_hash: u64,
    /// Global type version when this entry was populated
    type_version: u64,
    /// Cached field offset within the object's field storage
    field_offset: u32,
    /// Length of the attribute name (for collision disambiguation)
    name_len: u32,
}

impl FieldOffsetICEntry {
    const EMPTY: Self = Self {
        class_bits: 0,
        name_hash: 0,
        type_version: 0,
        field_offset: 0,
        name_len: 0,
    };
}

struct FieldOffsetIC {
    slots: [FieldOffsetICEntry; FIELD_OFFSET_IC_SIZE],
}

impl FieldOffsetIC {
    const fn new() -> Self {
        Self {
            slots: [FieldOffsetICEntry::EMPTY; FIELD_OFFSET_IC_SIZE],
        }
    }

    /// FNV-1a hash of attr name bytes — same family as `AttrNameCache::slot_index`.
    #[inline]
    fn hash_name(bytes: &[u8]) -> u64 {
        let mut h: u64 = 0xcbf2_9ce4_8422_2325;
        for &b in bytes {
            h ^= b as u64;
            h = h.wrapping_mul(0x0100_0000_01b3);
        }
        h
    }

    #[inline]
    fn slot_index(class_bits: u64, name_hash: u64) -> usize {
        // Mix class identity with name hash to spread across slots.
        let mixed = class_bits.wrapping_mul(0x9e37_79b9_7f4a_7c15) ^ name_hash;
        (mixed as usize) & (FIELD_OFFSET_IC_SIZE - 1)
    }

    /// Try to look up a cached field offset.  Returns `Some(offset)` on hit.
    #[inline]
    fn lookup(&self, class_bits: u64, name_bytes: &[u8], current_version: u64) -> Option<usize> {
        let name_hash = Self::hash_name(name_bytes);
        let idx = Self::slot_index(class_bits, name_hash);
        let entry = &self.slots[idx];
        if entry.class_bits == class_bits
            && entry.name_hash == name_hash
            && entry.type_version == current_version
            && entry.name_len == name_bytes.len() as u32
        {
            Some(entry.field_offset as usize)
        } else {
            None
        }
    }

    /// Populate (or overwrite) a cache slot after a successful field-offset
    /// resolution via the sealed inferred-field projection.
    #[inline]
    fn insert(
        &mut self,
        class_bits: u64,
        name_bytes: &[u8],
        current_version: u64,
        field_offset: usize,
    ) {
        let name_hash = Self::hash_name(name_bytes);
        let idx = Self::slot_index(class_bits, name_hash);
        self.slots[idx] = FieldOffsetICEntry {
            class_bits,
            name_hash,
            type_version: current_version,
            field_offset: field_offset as u32,
            name_len: name_bytes.len() as u32,
        };
    }

    fn clear(&mut self) {
        self.slots = [FieldOffsetICEntry::EMPTY; FIELD_OFFSET_IC_SIZE];
    }
}

thread_local! {
    static ATTR_NAME_TLS: RefCell<AttrNameCache> = const { RefCell::new(AttrNameCache::new()) };
    static DESCRIPTOR_CACHE_TLS: RefCell<Option<DescriptorCacheEntry>> = const { RefCell::new(None) };
    static FIELD_OFFSET_IC_TLS: RefCell<FieldOffsetIC> = const { RefCell::new(FieldOffsetIC::new()) };
}

/// Detach this thread's owned attribute-cache edges and report whether any existed.
/// The field-offset cache is non-owning and therefore does not affect the result.
pub(crate) fn clear_attr_tls_caches(_py: &PyToken<'_>) -> bool {
    crate::gil_assert();
    let attr_entries = ATTR_NAME_TLS
        .try_with(|cell| cell.borrow_mut().take_entries())
        .ok();
    let descriptor_entry = DESCRIPTOR_CACHE_TLS
        .try_with(|cell| cell.borrow_mut().take())
        .ok()
        .flatten();
    let _ = FIELD_OFFSET_IC_TLS.try_with(|cell| {
        cell.borrow_mut().clear();
    });

    // Every TLS borrow is gone before a release can run Python and repopulate
    // one of these caches. The outer shutdown quiescence loop observes that
    // reentry as owned work on its next pass.
    let mut detached = false;
    if let Some(entries) = attr_entries {
        for entry in entries.into_iter().flatten() {
            detached = true;
            dec_ref_bits(_py, entry.bits);
        }
    }
    if let Some(entry) = descriptor_entry {
        detached = true;
        entry.release(_py);
    }
    detached
}

/// Probe the field-offset IC for a cached (class, attr) -> offset mapping.
/// Returns `Some(offset)` on hit, `None` on miss.
#[inline]
pub(crate) fn field_offset_ic_lookup(
    class_bits: u64,
    attr_name_bytes: &[u8],
    current_version: u64,
) -> Option<usize> {
    FIELD_OFFSET_IC_TLS.with(|cell| {
        cell.borrow()
            .lookup(class_bits, attr_name_bytes, current_version)
    })
}

/// Populate the field-offset IC after a slow-path resolution.
#[inline]
pub(crate) fn field_offset_ic_insert(
    class_bits: u64,
    attr_name_bytes: &[u8],
    current_version: u64,
    field_offset: usize,
) {
    let _ = FIELD_OFFSET_IC_TLS.try_with(|cell| {
        cell.borrow_mut()
            .insert(class_bits, attr_name_bytes, current_version, field_offset);
    });
}

pub(crate) fn debug_last_attr_name() -> Option<String> {
    // Return the first populated slot for debugging purposes.
    ATTR_NAME_TLS
        .try_with(|cell| {
            let cache = cell.borrow();
            if let Some(entry) = cache.slots.iter().flatten().next() {
                return Some(String::from_utf8_lossy(entry.key.as_slice()).into_owned());
            }
            None
        })
        .ok()
        .flatten()
}

// Attribute APIs transport boxed values on both success and failure. Keep this
// result unsigned so raise_exception selects boxed None, never the raw signed
// status sentinel used by numeric/status APIs.
pub(crate) fn attr_error(_py: &PyToken<'_>, type_label: impl AsRef<str>, attr_name: &str) -> u64 {
    crate::gil_assert();
    let msg = format!(
        "'{}' object has no attribute '{}'",
        type_label.as_ref(),
        attr_name
    );
    raise_exception(_py, "AttributeError", &msg)
}

/// CPython 3.13 added a trailing clause to the AttributeError raised when
/// SETTING (or deleting) an attribute on an object that has no `__dict__` and no
/// slot to hold it: `'X' object has no attribute 'Y' and no __dict__ for setting
/// new attributes`. The GET path keeps the bare `'X' object has no attribute
/// 'Y'` on every version, so this suffix is exclusive to the set/del-failure
/// path. Version-gate it via `runtime_target_at_least(3, 13)` so molt matches
/// CPython 3.12 (no suffix) and 3.13/3.14 (suffix) exactly.
fn setattr_no_dict_suffix(_py: &PyToken<'_>) -> &'static str {
    if crate::object::ops_sys::runtime_target_at_least(_py, 3, 13) {
        " and no __dict__ for setting new attributes"
    } else {
        ""
    }
}

/// [`attr_error_with_obj`] for the set/del-failure path: appends the
/// version-gated `and no __dict__ for setting new attributes` clause (3.13+) and
/// records the `name`/`obj` members on the raised `AttributeError`.
pub(crate) fn setattr_no_attr_error_with_obj(
    _py: &PyToken<'_>,
    type_label: impl AsRef<str>,
    attr_name: &str,
    obj_bits: u64,
) -> u64 {
    crate::gil_assert();
    let msg = format!(
        "'{}' object has no attribute '{}'{}",
        type_label.as_ref(),
        attr_name,
        setattr_no_dict_suffix(_py),
    );
    let res = raise_exception(_py, "AttributeError", &msg);
    let exc_bits = exception_last_bits_noinc(_py).unwrap_or_else(|| MoltObject::none().bits());
    if !obj_from_bits(exc_bits).is_none()
        && set_attribute_error_members(_py, exc_bits, attr_name, obj_bits).is_err()
    {
        return res;
    }
    res
}

fn set_attribute_error_members(
    _py: &PyToken<'_>,
    exc_bits: u64,
    attr_name: &str,
    obj_bits: u64,
) -> Result<(), &'static str> {
    crate::gil_assert();
    let name_ptr = alloc_string(_py, attr_name.as_bytes());
    if name_ptr.is_null() {
        return Err("failed to allocate AttributeError.name");
    };
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    let result = exception_typed_fields_replace_internal(
        _py,
        exc_bits,
        &[
            (ExceptionTypedField::AttributeErrorName, name_bits),
            (ExceptionTypedField::AttributeErrorObject, obj_bits),
        ],
    );
    dec_ref_bits(_py, name_bits);
    result
}

pub(crate) fn attr_error_with_obj(
    _py: &PyToken<'_>,
    type_label: impl AsRef<str>,
    attr_name: &str,
    obj_bits: u64,
) -> u64 {
    crate::gil_assert();
    let msg = format!(
        "'{}' object has no attribute '{}'",
        type_label.as_ref(),
        attr_name
    );
    let res = raise_exception(_py, "AttributeError", &msg);
    let exc_bits = exception_last_bits_noinc(_py).unwrap_or_else(|| MoltObject::none().bits());
    if !obj_from_bits(exc_bits).is_none()
        && set_attribute_error_members(_py, exc_bits, attr_name, obj_bits).is_err()
    {
        return res;
    }
    res
}

pub(crate) fn attr_error_with_message(_py: &PyToken<'_>, msg: &str) -> u64 {
    crate::gil_assert();
    raise_exception(_py, "AttributeError", msg)
}

pub(crate) fn attr_error_with_obj_message(
    _py: &PyToken<'_>,
    msg: &str,
    attr_name: &str,
    obj_bits: u64,
) -> u64 {
    crate::gil_assert();
    let res = raise_exception(_py, "AttributeError", msg);
    let exc_bits = exception_last_bits_noinc(_py).unwrap_or_else(|| MoltObject::none().bits());
    if !obj_from_bits(exc_bits).is_none()
        && set_attribute_error_members(_py, exc_bits, attr_name, obj_bits).is_err()
    {
        return res;
    }
    res
}

pub(crate) fn attr_name_bits_from_bytes(_py: &PyToken<'_>, slice: &[u8]) -> Option<u64> {
    crate::gil_assert();
    if let Some(bits) = ATTR_NAME_TLS.with(|cell| cell.borrow().lookup(slice)) {
        inc_ref_bits(_py, bits);
        return Some(bits);
    }
    let ptr = alloc_string(_py, slice);
    if ptr.is_null() {
        return None;
    }
    let bits = MoltObject::from_ptr(ptr).bits();
    let previous = ATTR_NAME_TLS.with(|cell| cell.borrow_mut().insert(_py, slice, bits));
    if let Some(previous) = previous {
        dec_ref_bits(_py, previous);
    }
    Some(bits)
}

pub(crate) fn raise_attr_name_type_error(_py: &PyToken<'_>, name_bits: u64) -> u64 {
    crate::gil_assert();
    let name_obj = obj_from_bits(name_bits);
    let msg = format!(
        "attribute name must be string, not '{}'",
        type_name(_py, name_obj)
    );
    raise_exception(_py, "TypeError", &msg)
}

pub(crate) fn exception_is_attribute_error(_py: &PyToken<'_>, exc_bits: u64) -> bool {
    crate::gil_assert();
    exception_matches_builtin_name(_py, exc_bits, "AttributeError")
}

pub(crate) fn clear_attribute_error_if_pending(_py: &PyToken<'_>) -> bool {
    crate::gil_assert();
    if !exception_pending(_py) {
        return false;
    }
    let exc_bits = molt_exception_last_pending();
    let is_attr = exception_is_attribute_error(_py, exc_bits);
    if is_attr {
        clear_exception(_py);
        dec_ref_bits(_py, exc_bits);
        return true;
    }
    dec_ref_bits(_py, exc_bits);
    false
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ModuleLookupPolicy {
    Default,
    Required,
}

unsafe fn module_attr_lookup_impl(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    attr_bits: u64,
    policy: ModuleLookupPolicy,
) -> Option<u64> {
    unsafe {
        crate::gil_assert();
        // Module subtypes share descriptor, declared-slot and namespace
        // precedence with other class-shaped instances. PEP 562 and lazy
        // annotations apply only after that ordinary lookup misses.
        let value = object_attr_lookup_raw(_py, ptr, attr_bits);
        if value.is_some() || exception_pending(_py) {
            return value;
        }
        let dict_bits = module_dict_bits(ptr);
        let dict_obj = obj_from_bits(dict_bits);
        let dict_ptr = dict_obj.as_ptr()?;
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return None;
        }
        let annotations_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.annotations_name,
            b"__annotations__",
        );
        if crate::object::ops_compare::string_storage_equal(attr_bits, annotations_name_bits) {
            if let Some(val_bits) = dict_get_in_place(_py, dict_ptr, annotations_name_bits) {
                inc_ref_bits(_py, val_bits);
                return Some(val_bits);
            }
            let res_bits = if pep649_enabled(_py) {
                let annotate_name_bits = intern_static_name(
                    _py,
                    &runtime_state(_py).interned.annotate_name,
                    b"__annotate__",
                );
                let annotate_bits = dict_get_in_place(_py, dict_ptr, annotate_name_bits)
                    .unwrap_or_else(|| MoltObject::none().bits());
                if !obj_from_bits(annotate_bits).is_none() {
                    let format_bits = MoltObject::from_int(1).bits();
                    let res_bits = call_callable1(_py, annotate_bits, format_bits);
                    if exception_pending(_py) {
                        return None;
                    }
                    let res_obj = obj_from_bits(res_bits);
                    let Some(res_ptr) = res_obj.as_ptr() else {
                        let msg = format!(
                            "__annotate__ returned non-dict of type '{}'",
                            type_name(_py, res_obj)
                        );
                        dec_ref_bits(_py, res_bits);
                        return raise_exception(_py, "TypeError", &msg);
                    };
                    if object_type_id(res_ptr) != TYPE_ID_DICT {
                        let msg = format!(
                            "__annotate__ returned non-dict of type '{}'",
                            type_name(_py, res_obj)
                        );
                        dec_ref_bits(_py, res_bits);
                        return raise_exception(_py, "TypeError", &msg);
                    }
                    res_bits
                } else {
                    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                    if dict_ptr.is_null() {
                        return None;
                    }
                    MoltObject::from_ptr(dict_ptr).bits()
                }
            } else {
                let dict_ptr = alloc_dict_with_pairs(_py, &[]);
                if dict_ptr.is_null() {
                    return None;
                }
                MoltObject::from_ptr(dict_ptr).bits()
            };
            let complete_name_bits =
                attr_name_bits_from_bytes(_py, b"__molt_module_complete__").unwrap_or(0);
            let mut cache = false;
            if complete_name_bits != 0 {
                if let Some(complete_bits) = dict_get_in_place(_py, dict_ptr, complete_name_bits) {
                    cache = is_truthy(_py, obj_from_bits(complete_bits));
                }
                dec_ref_bits(_py, complete_name_bits);
            }
            if cache {
                dict_set_in_place(_py, dict_ptr, annotations_name_bits, res_bits);
            }
            return Some(res_bits);
        }
        let annotate_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.annotate_name,
            b"__annotate__",
        );
        if crate::object::ops_compare::string_storage_equal(attr_bits, annotate_name_bits) {
            if let Some(val_bits) = dict_get_in_place(_py, dict_ptr, annotate_name_bits) {
                inc_ref_bits(_py, val_bits);
                return Some(val_bits);
            }
            let none_bits = MoltObject::none().bits();
            inc_ref_bits(_py, none_bits);
            return Some(none_bits);
        }
        if policy == ModuleLookupPolicy::Default {
            return None;
        }
        let getattr_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.getattr_name,
            b"__getattr__",
        );
        if !crate::object::ops_compare::string_storage_equal(attr_bits, getattr_name_bits)
            && let Some(getattr_bits) = dict_get_in_place(_py, dict_ptr, getattr_name_bits)
        {
            inc_ref_bits(_py, getattr_bits);
            let res_bits = call_callable1(_py, getattr_bits, attr_bits);
            dec_ref_bits(_py, getattr_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, res_bits);
                return None;
            }
            return Some(res_bits);
        }
        None
    }
}

pub(crate) unsafe fn module_attr_lookup(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    unsafe { module_attr_lookup_impl(_py, ptr, attr_bits, ModuleLookupPolicy::Required) }
}

pub(crate) unsafe fn module_attr_lookup_default(
    py: &PyToken<'_>,
    ptr: *mut u8,
    name: u64,
) -> Option<u64> {
    unsafe { module_attr_lookup_impl(py, ptr, name, ModuleLookupPolicy::Default) }
}

pub(crate) unsafe fn instance_bits_for_call(ptr: *mut u8) -> u64 {
    MoltObject::from_ptr(ptr).bits()
}

fn function_code_descriptor_bits(_py: &PyToken<'_>) -> u64 {
    init_atomic_bits(
        _py,
        &runtime_state(_py).special_cache.function_code_descriptor,
        || {
            let getter_ptr = alloc_function_obj(_py, fn_addr!(molt_function_get_code), 1);
            if getter_ptr.is_null() {
                return 0;
            }
            unsafe {
                let builtin_bits = builtin_classes(_py).builtin_function_or_method;
                if !crate::object::object_init_class_edge_unpublished(
                    _py,
                    getter_ptr,
                    builtin_bits,
                    ClassEdgeOwnership::Owned,
                ) {
                    dec_ref_bits(_py, MoltObject::from_ptr(getter_ptr).bits());
                    return 0;
                }
            }
            let getter_bits = MoltObject::from_ptr(getter_ptr).bits();
            let none_bits = MoltObject::none().bits();
            let prop_ptr = alloc_property_obj(_py, getter_bits, none_bits, none_bits);
            dec_ref_bits(_py, getter_bits);
            if prop_ptr.is_null() {
                return 0;
            }
            MoltObject::from_ptr(prop_ptr).bits()
        },
    )
}

fn function_globals_descriptor_bits(_py: &PyToken<'_>) -> u64 {
    init_atomic_bits(
        _py,
        &runtime_state(_py).special_cache.function_globals_descriptor,
        || {
            let getter_ptr = alloc_function_obj(_py, fn_addr!(molt_function_get_globals), 1);
            if getter_ptr.is_null() {
                return 0;
            }
            unsafe {
                let builtin_bits = builtin_classes(_py).builtin_function_or_method;
                if !crate::object::object_init_class_edge_unpublished(
                    _py,
                    getter_ptr,
                    builtin_bits,
                    ClassEdgeOwnership::Owned,
                ) {
                    dec_ref_bits(_py, MoltObject::from_ptr(getter_ptr).bits());
                    return 0;
                }
            }
            let getter_bits = MoltObject::from_ptr(getter_ptr).bits();
            let none_bits = MoltObject::none().bits();
            let prop_ptr = alloc_property_obj(_py, getter_bits, none_bits, none_bits);
            dec_ref_bits(_py, getter_bits);
            if prop_ptr.is_null() {
                return 0;
            }
            MoltObject::from_ptr(prop_ptr).bits()
        },
    )
}

fn weakref_callback_descriptor_bits(py: &PyToken<'_>) -> u64 {
    init_atomic_bits(
        py,
        &runtime_state(py).special_cache.weakref_callback_descriptor,
        || {
            let getter_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                py,
                fn_addr!(crate::molt_weakref_callback_get),
                2,
            );
            if getter_ptr.is_null() {
                return 0;
            }
            let getter = MoltObject::from_ptr(getter_ptr).bits();
            let Some(name) = attr_name_bits_from_bytes(py, b"__callback__") else {
                dec_ref_bits(py, getter);
                return 0;
            };
            let none = MoltObject::none().bits();
            let descriptor = crate::builtins::types::alloc_native_descriptor(
                py,
                crate::builtins::types::NativeDescriptorSpec {
                    flavor: crate::builtins::types::NativeDescriptorFlavor::GetSet,
                    operation: 0,
                    owner: builtin_classes(py).reference_type,
                    name,
                    doc: none,
                    getter,
                    setter: none,
                    deleter: none,
                },
            );
            dec_ref_bits(py, name);
            dec_ref_bits(py, getter);
            if exception_pending(py) { 0 } else { descriptor }
        },
    )
}

/// Publish the native ``ReferenceType.__callback__`` descriptor into the
/// canonical builtin class dictionary after the builtin-class anchor family
/// itself has been published.
///
/// The descriptor depends on the already-published native descriptor and builtin
/// function classes, so it cannot be constructed while the class graph is
/// still being assembled.  Keeping it only as a lookup-time synthetic value
/// made ``ReferenceType.__dict__`` and static descriptor inspection disagree
/// with ordinary MRO lookup.  This post-publication step leaves one class-dict
/// authority for get, set, delete, ``dir``, and introspection.
pub(crate) fn install_weakref_callback_descriptor(_py: &PyToken<'_>) -> bool {
    let descriptor_bits = weakref_callback_descriptor_bits(_py);
    if descriptor_bits == 0 || exception_pending(_py) {
        return false;
    }
    let reference_bits = builtin_classes(_py).reference_type;
    let Some(reference_ptr) = obj_from_bits(reference_bits).as_ptr() else {
        return false;
    };
    let dict_bits = unsafe { class_dict_bits(reference_ptr) };
    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
        return false;
    };
    if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
        return false;
    }
    let Some(name_bits) = attr_name_bits_from_bytes(_py, b"__callback__") else {
        return false;
    };
    unsafe {
        dict_set_in_place(_py, dict_ptr, name_bits, descriptor_bits);
    }
    dec_ref_bits(_py, name_bits);
    !exception_pending(_py)
}

/// Borrow one member of the actual declaring namespace. Native declarations
/// materialize only the requested name into that same dictionary.
pub(crate) unsafe fn class_namespace_lookup_raw(
    py: &PyToken<'_>,
    class: *mut u8,
    name: u64,
) -> Option<u64> {
    unsafe {
        crate::gil_assert();
        let dictionary = obj_from_bits(class_dict_bits(class)).as_ptr()?;
        if object_type_id(dictionary) != TYPE_ID_DICT {
            return None;
        }
        if let Some(value) = dict_get_in_place(py, dictionary, name) {
            return Some(value);
        }
        clear_attribute_error_if_pending(py);
        if exception_pending(py) {
            return None;
        }
        let bits = MoltObject::from_ptr(class).bits();
        if bits == builtin_classes(py).function {
            let spelling = string_obj_to_owned(obj_from_bits(name))?;
            let value = match spelling.as_str() {
                "__code__" => function_code_descriptor_bits(py),
                "__globals__" => function_globals_descriptor_bits(py),
                _ => 0,
            };
            if value != 0 {
                return Some(value);
            }
        }
        if crate::object::class_storage::class_declares(
            class,
            crate::object::class_storage::ClassDeclaration::NativeNamespacePublished,
        ) {
            return None;
        }
        let spelling = string_obj_to_owned(obj_from_bits(name))?;
        if is_builtin_class_bits(py, bits)
            || crate::builtins::exceptions::is_builtin_exception_class_bits(py, bits)
        {
            builtin_class_method_bits(py, bits, &spelling)
        } else {
            None
        }
    }
}

/// Raw MRO lookup returns a namespace-owned value without descriptor binding.
/// Consumers retaining it across user code must establish an owned reference.
pub(crate) unsafe fn class_namespace_lookup_mro(
    py: &PyToken<'_>,
    class: *mut u8,
    name: u64,
) -> Option<u64> {
    unsafe {
        crate::gil_assert();
        if let Some(mro) = class_mro_pinned(py, class) {
            for bits in mro.iter() {
                let Some(owner) = obj_from_bits(*bits).as_ptr() else {
                    continue;
                };
                if object_type_id(owner) != TYPE_ID_TYPE {
                    continue;
                }
                if let Some(value) = class_namespace_lookup_raw(py, owner, name) {
                    return Some(value);
                }
                if exception_pending(py) {
                    return None;
                }
            }
            return None;
        }
        let mut current = class;
        for _ in 0..=64 {
            if let Some(value) = class_namespace_lookup_raw(py, current, name) {
                return Some(value);
            }
            if exception_pending(py) {
                return None;
            }
            let bases = class_bases_vec(class_bases_bits(current));
            let next = obj_from_bits(*bases.first()?).as_ptr()?;
            if object_type_id(next) != TYPE_ID_TYPE || next == current {
                return None;
            }
            current = next;
        }
        None
    }
}

/// Runtime class metadata has a default __doc__ value; the underlying raw
/// namespace/MRO authority stays suitable for C lookup without synthesis.
pub(crate) unsafe fn class_attr_lookup_raw_mro(
    py: &PyToken<'_>,
    class: *mut u8,
    name: u64,
) -> Option<u64> {
    unsafe {
        class_namespace_lookup_mro(py, class, name).or_else(|| {
            (!exception_pending(py)
                && string_obj_to_owned(obj_from_bits(name)).as_deref() == Some("__doc__"))
            .then(|| MoltObject::none().bits())
        })
    }
}

/// Exact-string lookup over dict insertion order. Sealed maps have already
/// validated every key/value pair, so this performs neither hashing nor Python
/// equality and is safe for GC-adjacent physical-layout consumers.
unsafe fn field_offset_in_map(
    py: &PyToken<'_>,
    offsets_bits: u64,
    attr_bits: u64,
) -> Option<usize> {
    unsafe {
        let attr_ptr = obj_from_bits(attr_bits).as_ptr()?;
        if object_type_id(attr_ptr) != TYPE_ID_STRING {
            return None;
        }
        let offsets_ptr = obj_from_bits(offsets_bits).as_ptr()?;
        if object_type_id(offsets_ptr) != TYPE_ID_DICT {
            return None;
        }
        let name = std::slice::from_raw_parts(string_bytes(attr_ptr), string_len(attr_ptr));
        crate::object::ops::dict_get_str_bytes_borrowed(py, offsets_ptr, name)
            .and_then(|bits| obj_from_bits(bits).as_int())
            .and_then(|offset| usize::try_from(offset).ok())
    }
}

pub(crate) unsafe fn class_field_offset(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    attr_bits: u64,
) -> Option<usize> {
    unsafe {
        crate::gil_assert();
        crate::object::class_finish_definition(_py, class_ptr).ok()?;
        field_offset_in_map(
            _py,
            crate::object::layout::class_field_offsets_bits(class_ptr),
            attr_bits,
        )
    }
}

/// Ordinary attribute fallback admits inferred storage only. Declared slots
/// execute their namespace descriptor; rebinding/deleting that descriptor does
/// not leave an independent physical-slot lookup path behind.
pub(crate) unsafe fn class_inferred_field_offset(
    py: &PyToken<'_>,
    class: *mut u8,
    name: u64,
) -> Option<usize> {
    unsafe {
        crate::object::class_finish_definition(py, class).ok()?;
        let offset = field_offset_in_map(
            py,
            crate::object::layout::class_field_offsets_bits(class),
            name,
        )?;
        crate::object::class_layout::field_at_offset(class, offset)
            .filter(|field| field.kind.is_inferred())
            .map(|field| field.offset)
    }
}

/// Slot names are validated strings in a private immutable tuple. Compare their
/// physical text without invoking user equality or rereading __slots__.
unsafe fn slot_declaration_contains(names: u64, attr_bits: u64) -> bool {
    unsafe {
        let Some(names) = obj_from_bits(names).as_ptr() else {
            return false;
        };
        crate::object::seq_access::with_immutable_tuple_slice(names, |names| {
            names
                .iter()
                .copied()
                .any(|name| crate::object::ops_compare::string_storage_equal(name, attr_bits))
        })
        .unwrap_or(false)
    }
}

pub(crate) unsafe fn class_own_slot_field_offset(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    attr_bits: u64,
) -> Option<usize> {
    unsafe {
        crate::gil_assert();
        if class_ptr.is_null() || object_type_id(class_ptr) != TYPE_ID_TYPE {
            return None;
        }
        let crate::object::layout::ClassSlotDeclaration::Names(names) =
            crate::object::layout::class_slot_declaration(class_ptr)
        else {
            return None;
        };
        if !slot_declaration_contains(names, attr_bits) {
            return None;
        }
        class_field_offset(_py, class_ptr, attr_bits)
    }
}

/// Probe a type slot without invoking its descriptor. Protocol admission must
/// not bind a method that is only observed when the operation executes.
pub(crate) unsafe fn has_special_method(py: &PyToken<'_>, bits: u64, name: &[u8]) -> bool {
    unsafe {
        if let Some(instance) = obj_from_bits(bits).as_ptr()
            && crate::object_type_id(instance) == crate::TYPE_ID_FOREIGN
        {
            use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
            let native = std::ptr::with_exposed_provenance_mut(
                crate::object::foreign::foreign_ptr_from_obj(instance),
            );
            let Some(name_bits) = attr_name_bits_from_bytes(py, name) else {
                return false;
            };
            let c_name = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(name_bits);
            let result = if c_name.is_null() {
                Err(molt_cpython_abi::ErrorIndicatorSet)
            } else {
                molt_cpython_abi::api::object::has_type_special(native, c_name)
            };
            if result.is_err() {
                crate::cpython_abi_hooks::propagate_native_failure(
                    py,
                    "special-method presence lookup",
                );
            }
            molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, name_bits));
            return result.unwrap_or(false);
        }
        let Some(class) = obj_from_bits(type_of_bits(py, bits)).as_ptr() else {
            return false;
        };
        let Some(name_bits) = attr_name_bits_from_bytes(py, name) else {
            return false;
        };
        let present = class_attr_lookup_raw_mro(py, class, name_bits).is_some();
        dec_ref_bits(py, name_bits);
        present
    }
}

/// Special methods bypass the instance dictionary and __getattribute__.
pub(crate) unsafe fn lookup_special_method(
    py: &PyToken<'_>,
    bits: u64,
    name: &[u8],
) -> Option<u64> {
    unsafe {
        let name = attr_name_bits_from_bytes(py, name)?;
        let method = lookup_special_method_bits(py, bits, name);
        molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, name));
        method
    }
}

/// The same type-only protocol for callers that already own an interned name.
/// Presence is carried by Option, including a descriptor result of float +0.0.
pub(crate) unsafe fn lookup_special_method_bits(
    py: &PyToken<'_>,
    bits: u64,
    name: u64,
) -> Option<u64> {
    unsafe {
        let instance = obj_from_bits(bits).as_ptr()?;
        if crate::object_type_id(instance) == crate::TYPE_ID_FOREIGN {
            use molt_cpython_abi::bridge::GLOBAL_BRIDGE;
            let native = std::ptr::with_exposed_provenance_mut(
                crate::object::foreign::foreign_ptr_from_obj(instance),
            );
            let c_name = GLOBAL_BRIDGE.handle_to_borrowed_pyobj(name);
            if c_name.is_null() {
                crate::cpython_abi_hooks::propagate_native_failure(
                    py,
                    "special-method name projection",
                );
                return None;
            }
            return match molt_cpython_abi::api::object::lookup_type_special(native, c_name) {
                Ok(Some(method)) => {
                    let result = GLOBAL_BRIDGE.molt_value_for_pyobj(method.as_ptr());
                    if result.is_none() {
                        crate::cpython_abi_hooks::propagate_native_failure(
                            py,
                            "special-method result projection",
                        );
                    }
                    result
                }
                Ok(None) => None,
                Err(molt_cpython_abi::ErrorIndicatorSet) => {
                    crate::cpython_abi_hooks::propagate_native_failure(
                        py,
                        "special-method descriptor lookup",
                    );
                    None
                }
            };
        }
        let class = obj_from_bits(type_of_bits(py, bits)).as_ptr()?;
        let method = class_attr_lookup(py, class, class, Some(instance), name);
        if exception_pending(py) {
            if let Some(method) = method {
                molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, method));
            }
            return None;
        }
        method
    }
}

/// Async slot wrappers in CPython before 3.14 replace descriptor lookup
/// failures. Keep that target-version rule separate from generic special lookup.
pub(crate) unsafe fn lookup_async_special_method(
    py: &PyToken<'_>,
    bits: u64,
    name: &[u8],
) -> Option<u64> {
    unsafe {
        let method = lookup_special_method(py, bits, name);
        if method.is_none()
            && exception_pending(py)
            && !crate::object::ops_sys::runtime_target_at_least(py, 3, 14)
        {
            crate::clear_exception(py);
            let message = format!(
                "object {} does not have {} method",
                type_name(py, obj_from_bits(bits)),
                String::from_utf8_lossy(name)
            );
            return raise_exception::<Option<u64>>(py, "AttributeError", &message);
        }
        method
    }
}

pub(crate) unsafe fn is_iterator_bits(_py: &PyToken<'_>, bits: u64) -> bool {
    unsafe {
        crate::gil_assert();
        let Some(ptr) = maybe_ptr_from_bits(bits) else {
            return false;
        };
        match object_type_id(ptr) {
            TYPE_ID_ITER | TYPE_ID_GENERATOR | TYPE_ID_ENUMERATE | TYPE_ID_CALL_ITER
            | TYPE_ID_REVERSED | TYPE_ID_ZIP | TYPE_ID_MAP | TYPE_ID_FILTER | TYPE_ID_GLOB_ITER
            | TYPE_ID_FILE_HANDLE => return true,
            _ => {}
        }
        has_special_method(_py, bits, b"__next__")
    }
}

pub(crate) fn descriptor_cache_lookup(
    _py: &PyToken<'_>,
    class_bits: u64,
    attr_bits: u64,
    version: u64,
) -> Option<DescriptorSnapshot> {
    crate::gil_assert();
    let attr_ptr = obj_from_bits(attr_bits).as_ptr()?;
    let attr_bytes = unsafe {
        if object_type_id(attr_ptr) != TYPE_ID_STRING {
            return None;
        }
        std::slice::from_raw_parts(string_bytes(attr_ptr), string_len(attr_ptr))
    };
    DESCRIPTOR_CACHE_TLS.with(|cell| {
        cell.borrow()
            .as_ref()
            .filter(|entry| {
                entry.snapshot.class_bits == class_bits
                    && entry.version == version
                    && entry.attr_name == attr_bytes
            })
            .map(|entry| entry.snapshot.retained(_py))
    })
}

pub(crate) fn descriptor_cache_store(
    _py: &PyToken<'_>,
    class_bits: u64,
    attr_bits: u64,
    version: u64,
    data_desc_bits: Option<u64>,
    class_attr_bits: Option<u64>,
) {
    crate::gil_assert();
    let Some(attr_name) = string_obj_bytes(obj_from_bits(attr_bits)) else {
        return;
    };
    let entry = DescriptorCacheEntry {
        attr_name,
        version,
        snapshot: DescriptorSnapshot::retain(_py, class_bits, data_desc_bits, class_attr_bits),
    };
    let old_entry = DESCRIPTOR_CACHE_TLS.with(|cell| cell.borrow_mut().replace(entry));
    if let Some(old_entry) = old_entry {
        old_entry.release(_py);
    }
}

/// Retain the initial class lookup across callbacks in the instance tier.
/// Both a hit and a miss are a snapshot: dictionary equality cannot replace
/// the selected descriptor or cause this operation to search the MRO again.
unsafe fn with_class_descriptor_snapshot<R>(
    py: &PyToken<'_>,
    class_bits: u64,
    attr_bits: u64,
    lookup: impl FnOnce(Option<&DescriptorSnapshot>) -> R,
) -> R {
    unsafe {
        let snapshot = if class_bits != 0
            && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
            && object_type_id(class_ptr) == TYPE_ID_TYPE
        {
            let version = class_layout_version_bits(class_ptr);
            Some(
                match descriptor_cache_lookup(py, class_bits, attr_bits, version) {
                    Some(entry) => entry,
                    None => {
                        let Some(attr_ptr) = obj_from_bits(attr_bits).as_ptr() else {
                            return lookup(None);
                        };
                        if object_type_id(attr_ptr) != TYPE_ID_STRING {
                            return lookup(None);
                        }
                        let mut entry = DescriptorSnapshot::retain(py, class_bits, None, None);
                        if let Some(bits) = class_attr_lookup_raw_mro(py, class_ptr, attr_bits) {
                            // Pin before descriptor classification or cache retirement
                            // can run code that replaces the class's reference.
                            inc_ref_bits(py, bits);
                            entry.class_attr_bits = Some(bits);
                            if descriptor_is_data(py, bits) {
                                entry.data_desc_bits = entry.class_attr_bits.take();
                            }
                        }
                        if !exception_pending(py) {
                            molt_cpython_abi::api::errors::with_preserved_error(|| {
                                descriptor_cache_store(
                                    py,
                                    class_bits,
                                    attr_bits,
                                    version,
                                    entry.data_desc_bits,
                                    entry.class_attr_bits,
                                );
                            });
                        }
                        entry
                    }
                },
            )
        } else {
            None
        };
        let result = lookup(snapshot.as_ref());
        if let Some(snapshot) = snapshot {
            molt_cpython_abi::api::errors::with_preserved_error(|| {
                snapshot.release(py);
            });
        }
        result
    }
}

/// Special methods belong to the receiver's type, including the metaclass of
/// a class object. Never inspect the receiver's own namespace for this protocol.
unsafe fn descriptor_type_ptr(_py: &PyToken<'_>, val_bits: u64) -> Option<*mut u8> {
    unsafe {
        let ptr = obj_from_bits(type_of_bits(_py, val_bits)).as_ptr()?;
        (object_type_id(ptr) == TYPE_ID_TYPE).then_some(ptr)
    }
}

/// Query the same binding protocol used by bind_descriptor without binding or
/// consulting the descriptor instance's attributes.
pub(crate) unsafe fn descriptor_has_get(py: &PyToken<'_>, value: u64) -> bool {
    unsafe {
        let Some(pointer) = maybe_ptr_from_bits(value) else {
            return false;
        };
        match object_type_id(pointer) {
            crate::TYPE_ID_FOREIGN => molt_cpython_abi::bridge::molt_foreign_descriptor_has_get(
                crate::object::foreign::foreign_ptr_from_obj(pointer),
            ),
            TYPE_ID_FUNCTION => {
                crate::builtins::functions::native_callable::NativeCallableKind::from_class(
                    py,
                    object_class_bits(pointer),
                )
                .is_none_or(|kind| kind.is_descriptor())
            }
            TYPE_ID_NATIVE_DESCRIPTOR
            | TYPE_ID_CLASSMETHOD
            | TYPE_ID_STATICMETHOD
            | TYPE_ID_PROPERTY => true,
            _ => descriptor_type_ptr(py, value).is_some_and(|owner| {
                class_attr_lookup_raw_mro(py, owner, DescriptorHook::Get.name_bits(py)).is_some()
            }),
        }
    }
}

pub(crate) unsafe fn descriptor_is_data(_py: &PyToken<'_>, val_bits: u64) -> bool {
    unsafe {
        crate::gil_assert();
        let Some(val_ptr) = maybe_ptr_from_bits(val_bits) else {
            return false;
        };
        if object_type_id(val_ptr) == crate::TYPE_ID_FOREIGN {
            return molt_cpython_abi::bridge::molt_foreign_descriptor_is_data(
                crate::object::foreign::foreign_ptr_from_obj(val_ptr),
            );
        }
        if matches!(
            object_type_id(val_ptr),
            TYPE_ID_PROPERTY | TYPE_ID_NATIVE_DESCRIPTOR
        ) {
            return true;
        }
        let Some(owner) = descriptor_type_ptr(_py, val_bits) else {
            return false;
        };
        let set_bits = intern_static_name(_py, &runtime_state(_py).interned.set_name, b"__set__");
        let del_bits =
            intern_static_name(_py, &runtime_state(_py).interned.delete_name, b"__delete__");
        class_attr_lookup_raw_mro(_py, owner, set_bits).is_some()
            || class_attr_lookup_raw_mro(_py, owner, del_bits).is_some()
    }
}

/// Optional normal lookup differs only in its AttributeError policy. Physical
/// storage and dictionary availability never select whether user hooks run.
/// Implicit type protocols use lookup_special_method instead.
pub(crate) unsafe fn attr_lookup_ptr_allow_missing(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    unsafe {
        crate::gil_assert();
        let res = crate::builtins::attributes::attr_lookup_ptr(_py, obj_ptr, attr_bits);
        if matches!(
            std::env::var("MOLT_TRACE_INIT_SUBCLASS").ok().as_deref(),
            Some("1")
        ) && string_obj_to_owned(obj_from_bits(attr_bits)).as_deref()
            == Some("__init_subclass__")
        {
            match res {
                Some(bits) => {
                    let obj = obj_from_bits(bits);
                    eprintln!(
                        "molt init_subclass allow_missing res_bits=0x{:x} none={} ptr={}",
                        bits,
                        obj.is_none(),
                        obj.as_ptr().is_some(),
                    );
                }
                None => {
                    eprintln!("molt init_subclass allow_missing res=None");
                }
            }
        }
        if exception_pending(_py) {
            if let Some(result) = res {
                molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(_py, result));
            }
            let _ = clear_attribute_error_if_pending(_py);
            return None;
        }
        res
    }
}

/// Actual callable identity governs both materialized and immediate binding.
unsafe fn function_descriptor_receiver(
    py: &PyToken<'_>,
    function_ptr: *mut u8,
    owner: Option<u64>,
    instance: Option<u64>,
) -> Result<Option<crate::builtins::functions::native_callable::NativeDescriptorReceiver>, ()> {
    unsafe {
        if let Some(kind) =
            crate::builtins::functions::native_callable::NativeCallableKind::from_class(
                py,
                object_class_bits(function_ptr),
            )
        {
            crate::builtins::functions::native_callable::native_descriptor_receiver(
                py,
                function_ptr,
                kind,
                crate::builtins::functions::native_callable::NativeDescriptorContext::Binding,
                owner,
                instance,
            )
        } else {
            Ok(instance.map(
                crate::builtins::functions::native_callable::NativeDescriptorReceiver::borrowed,
            ))
        }
    }
}

/// Bind a descriptor without suppressing its exception. Both instance and owner
/// preserve exact tagged values; None means that argument is absent. Attribute
/// fallback belongs to the complete lookup transaction, never this primitive.
pub(crate) unsafe fn descriptor_bind(
    py: &PyToken<'_>,
    value: u64,
    owner: Option<u64>,
    instance: Option<u64>,
) -> Option<u64> {
    unsafe { bind_descriptor(py, value, owner, instance, DescriptorCallPolicy::Required) }
}

/// These contracts apply only to binding, never to errors from the called
/// Python body. CPython changed optional special-method lookup in 3.14.
#[derive(Clone, Copy)]
pub(crate) enum DescriptorCallPolicy {
    Required,
    Optional,
    RichComparison,
}

fn descriptor_hook_result(
    _py: &PyToken<'_>,
    result_bits: u64,
    policy: DescriptorCallPolicy,
) -> Option<u64> {
    if !exception_pending(_py) {
        return Some(result_bits);
    }
    dec_ref_bits(_py, result_bits);
    let suppress = match policy {
        DescriptorCallPolicy::Required => false,
        DescriptorCallPolicy::Optional => {
            crate::object::ops_sys::runtime_target_at_least(_py, 3, 14)
                && clear_attribute_error_if_pending(_py)
        }
        DescriptorCallPolicy::RichComparison => {
            if crate::object::ops_sys::runtime_target_at_least(_py, 3, 14) {
                clear_attribute_error_if_pending(_py)
            } else {
                clear_exception(_py);
                true
            }
        }
    };
    if suppress {
        None
    } else {
        Some(MoltObject::none().bits())
    }
}

unsafe fn bind_descriptor(
    _py: &PyToken<'_>,
    val_bits: u64,
    owner: Option<u64>,
    instance_bits: Option<u64>,
    exception_policy: DescriptorCallPolicy,
) -> Option<u64> {
    unsafe {
        crate::gil_assert();
        if exception_pending(_py) {
            return Some(MoltObject::none().bits());
        }
        let Some(val_ptr) = maybe_ptr_from_bits(val_bits) else {
            inc_ref_bits(_py, val_bits);
            return Some(val_bits);
        };
        // Descriptor binding is the canonical boundary where class-dict/cache
        // descriptor values can run arbitrary user code through property getters
        // or `__get__`. Own the descriptor for this full operation so class
        // mutation during the hook cannot invalidate the borrowed lookup source.
        inc_ref_bits(_py, val_bits);
        if let Some(instance) = instance_bits {
            inc_ref_bits(_py, instance);
        }
        if let Some(owner) = owner {
            inc_ref_bits(_py, owner);
        }
        let result = match object_type_id(val_ptr) {
            TYPE_ID_FUNCTION => {
                match function_descriptor_receiver(_py, val_ptr, owner, instance_bits) {
                    Ok(Some(receiver)) => Some(molt_bound_method_new(val_bits, receiver.bits())),
                    Ok(None) => {
                        inc_ref_bits(_py, val_bits);
                        Some(val_bits)
                    }
                    Err(()) => Some(MoltObject::none().bits()),
                }
            }
            TYPE_ID_NATIVE_DESCRIPTOR => {
                let value = crate::builtins::types::native_descriptor_get(
                    _py,
                    val_bits,
                    instance_bits,
                    owner,
                );
                descriptor_hook_result(_py, value, exception_policy)
            }
            crate::TYPE_ID_FOREIGN => {
                let result = molt_cpython_abi::bridge::molt_foreign_descriptor_get(
                    crate::object::foreign::foreign_ptr_from_obj(val_ptr),
                    instance_bits,
                    owner,
                );
                match result.decode() {
                    molt_cpython_abi::hooks::DecodedHandleResult::Ok(bits) => {
                        descriptor_hook_result(_py, bits, exception_policy)
                    }
                    molt_cpython_abi::hooks::DecodedHandleResult::Missing => {
                        inc_ref_bits(_py, val_bits);
                        Some(val_bits)
                    }
                    molt_cpython_abi::hooks::DecodedHandleResult::Error => {
                        crate::cpython_abi_hooks::propagate_native_failure(
                            _py,
                            "native descriptor",
                        );
                        descriptor_hook_result(_py, MoltObject::none().bits(), exception_policy)
                    }
                }
            }
            type_id @ (TYPE_ID_CLASSMETHOD | TYPE_ID_STATICMETHOD | TYPE_ID_PROPERTY)
                if crate::builtins::types::wrappers::wrapper_uses_default_method(
                    _py,
                    val_ptr,
                    b"__get__",
                    match type_id {
                        TYPE_ID_CLASSMETHOD => fn_key!(crate::molt_classmethod_get),
                        TYPE_ID_STATICMETHOD => fn_key!(crate::molt_staticmethod_get),
                        _ => fn_key!(crate::molt_property_get),
                    },
                ) =>
            {
                let value = crate::builtins::types::wrappers::wrapper_get(
                    _py,
                    val_bits,
                    instance_bits,
                    owner,
                );
                descriptor_hook_result(_py, value, exception_policy)
            }
            _ if exception_pending(_py) => Some(MoltObject::none().bits()),
            _ => {
                let inst_bits = instance_bits.unwrap_or_else(|| MoltObject::none().bits());
                let owner_bits = owner.unwrap_or_else(|| MoltObject::none().bits());
                let result = call_descriptor_method(
                    _py,
                    val_bits,
                    DescriptorHook::Get,
                    DescriptorArguments::Two(inst_bits, owner_bits),
                );
                match result {
                    Some(bits) => descriptor_hook_result(_py, bits, exception_policy),
                    None if exception_pending(_py) => Some(MoltObject::none().bits()),
                    None => {
                        inc_ref_bits(_py, val_bits);
                        Some(val_bits)
                    }
                }
            }
        };
        molt_cpython_abi::api::errors::with_preserved_error(|| {
            if let Some(owner) = owner {
                dec_ref_bits(_py, owner);
            }
            if let Some(instance) = instance_bits {
                dec_ref_bits(_py, instance);
            }
            dec_ref_bits(_py, val_bits);
        });
        result
    }
}

/// Invoke a descriptor with one explicit argument, retaining the same receiver
/// policy as `descriptor_bind` without allocating a transient bound method for
/// exact functions. Raw lookup values are borrowed; both invocation paths pin
/// the callable before user code can mutate its lookup source.
pub(crate) unsafe fn descriptor_call1(
    _py: &PyToken<'_>,
    val_bits: u64,
    owner_ptr: *mut u8,
    instance_bits: Option<u64>,
    arg_bits: u64,
) -> Option<u64> {
    unsafe {
        descriptor_special_call1(
            _py,
            val_bits,
            owner_ptr,
            instance_bits,
            arg_bits,
            DescriptorCallPolicy::Required,
        )
    }
}

pub(crate) unsafe fn descriptor_special_call1(
    _py: &PyToken<'_>,
    val_bits: u64,
    owner_ptr: *mut u8,
    instance_bits: Option<u64>,
    arg_bits: u64,
    policy: DescriptorCallPolicy,
) -> Option<u64> {
    unsafe {
        descriptor_invoke(
            _py,
            val_bits,
            owner_ptr,
            instance_bits,
            DescriptorArguments::One(arg_bits),
            policy,
        )
    }
}

#[derive(Clone, Copy)]
enum DescriptorArguments {
    One(u64),
    Two(u64, u64),
}

/// Two-argument counterpart for special methods such as __setattr__. Both
/// arities share the same callable ownership, receiver and exception policy.
pub(crate) unsafe fn descriptor_call2(
    py: &PyToken<'_>,
    value: u64,
    owner_ptr: *mut u8,
    instance: Option<u64>,
    first: u64,
    second: u64,
) -> Option<u64> {
    unsafe {
        descriptor_invoke(
            py,
            value,
            owner_ptr,
            instance,
            DescriptorArguments::Two(first, second),
            DescriptorCallPolicy::Required,
        )
    }
}

impl DescriptorArguments {
    unsafe fn call(self, _py: &PyToken<'_>, callable: u64) -> u64 {
        unsafe {
            match self {
                Self::One(arg) => call_callable1(_py, callable, arg),
                Self::Two(first, second) => call_callable2(_py, callable, first, second),
            }
        }
    }

    unsafe fn call_with_receiver(self, _py: &PyToken<'_>, callable: u64, receiver: u64) -> u64 {
        unsafe {
            match self {
                Self::One(arg) => call_callable2(_py, callable, receiver, arg),
                Self::Two(first, second) => call_callable3(_py, callable, receiver, first, second),
            }
        }
    }
}

unsafe fn descriptor_invoke(
    _py: &PyToken<'_>,
    val_bits: u64,
    owner_ptr: *mut u8,
    instance_bits: Option<u64>,
    args: DescriptorArguments,
    policy: DescriptorCallPolicy,
) -> Option<u64> {
    unsafe {
        crate::gil_assert();
        if let Some(val_ptr) = maybe_ptr_from_bits(val_bits)
            && object_type_id(val_ptr) == TYPE_ID_FUNCTION
        {
            inc_ref_bits(_py, val_bits);
            let result = match function_descriptor_receiver(
                _py,
                val_ptr,
                (!owner_ptr.is_null()).then(|| MoltObject::from_ptr(owner_ptr).bits()),
                instance_bits,
            ) {
                Ok(Some(receiver)) => {
                    // Match the receiver lifetime a materialized bound method
                    // would own, even if the callback drops its original owner.
                    inc_ref_bits(_py, receiver.bits());
                    let result = args.call_with_receiver(_py, val_bits, receiver.bits());
                    dec_ref_bits(_py, receiver.bits());
                    result
                }
                Ok(None) => args.call(_py, val_bits),
                Err(()) => MoltObject::none().bits(),
            };
            dec_ref_bits(_py, val_bits);
            return Some(result);
        }
        // Descriptor binding may recurse before entering a Python function.
        let _guard = crate::state::recursion::RecursionGuard::enter_with_message(
            _py,
            "maximum recursion depth exceeded while binding a descriptor",
        )?;
        let bound_bits = bind_descriptor(
            _py,
            val_bits,
            (!owner_ptr.is_null()).then(|| MoltObject::from_ptr(owner_ptr).bits()),
            instance_bits,
            policy,
        )?;
        // Binding itself can fail. Never invoke the error sentinel and replace
        // the descriptor's exception with an incidental not-callable error.
        if exception_pending(_py) {
            dec_ref_bits(_py, bound_bits);
            return Some(MoltObject::none().bits());
        }
        let result = args.call(_py, bound_bits);
        dec_ref_bits(_py, bound_bits);
        Some(result)
    }
}

#[derive(Clone, Copy)]
enum DescriptorHook {
    Get,
    Set,
    Delete,
}

impl DescriptorHook {
    fn name_bits(self, py: &PyToken<'_>) -> u64 {
        let names = &runtime_state(py).interned;
        match self {
            Self::Get => intern_static_name(py, &names.get_name, b"__get__"),
            Self::Set => intern_static_name(py, &names.set_name, b"__set__"),
            Self::Delete => intern_static_name(py, &names.delete_name, b"__delete__"),
        }
    }
}

/// Resolve hooks only on the descriptor's type. CPython deliberately calls
/// `__get__` raw with explicit descriptor self, but binds mutation hooks first.
/// Keep that protocol distinction here, independent of function arity or caller.
unsafe fn call_descriptor_method(
    _py: &PyToken<'_>,
    descriptor: u64,
    hook: DescriptorHook,
    args: DescriptorArguments,
) -> Option<u64> {
    unsafe {
        let owner = descriptor_type_ptr(_py, descriptor)?;
        let owner_bits = MoltObject::from_ptr(owner).bits();
        inc_ref_bits(_py, owner_bits);
        let result = match class_attr_lookup_raw_mro(_py, owner, hook.name_bits(_py)) {
            Some(raw) if !exception_pending(_py) => match hook {
                DescriptorHook::Get => {
                    inc_ref_bits(_py, raw);
                    let result = args.call_with_receiver(_py, raw, descriptor);
                    dec_ref_bits(_py, raw);
                    Some(result)
                }
                DescriptorHook::Set | DescriptorHook::Delete => descriptor_invoke(
                    _py,
                    raw,
                    owner,
                    Some(descriptor),
                    args,
                    DescriptorCallPolicy::Required,
                ),
            },
            _ => None,
        };
        dec_ref_bits(_py, owner_bits);
        result
    }
}

#[derive(Clone, Copy)]
pub(crate) enum DescriptorMutation {
    Set(u64),
    Delete,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum DescriptorMutationOutcome {
    Applied,
    NotDescriptor,
    Error,
}

/// Execute one descriptor-owned write. Contextual attribute diagnostics stay at
/// the attribute boundary; hook lookup, callable binding and all callback pins
/// are shared by ordinary, dataclass and metaclass consumers.
pub(crate) unsafe fn descriptor_mutate(
    _py: &PyToken<'_>,
    descriptor: u64,
    instance: u64,
    mutation: DescriptorMutation,
) -> DescriptorMutationOutcome {
    unsafe {
        crate::gil_assert();
        if exception_pending(_py) {
            return DescriptorMutationOutcome::Error;
        }
        let Some(ptr) = maybe_ptr_from_bits(descriptor) else {
            return DescriptorMutationOutcome::NotDescriptor;
        };
        inc_ref_bits(_py, descriptor);
        inc_ref_bits(_py, instance);
        let outcome = if object_type_id(ptr) == crate::TYPE_ID_FOREIGN {
            let value = match mutation {
                DescriptorMutation::Set(value) => Some(value),
                DescriptorMutation::Delete => None,
            };
            let result = molt_cpython_abi::bridge::molt_foreign_descriptor_set(
                crate::object::foreign::foreign_ptr_from_obj(ptr),
                instance,
                value,
            );
            match result.decode() {
                molt_cpython_abi::hooks::DecodedHandleResult::Ok(bits) => {
                    crate::call::discard_owned_call_result(_py, bits);
                    DescriptorMutationOutcome::Applied
                }
                molt_cpython_abi::hooks::DecodedHandleResult::Missing => {
                    DescriptorMutationOutcome::NotDescriptor
                }
                molt_cpython_abi::hooks::DecodedHandleResult::Error => {
                    crate::cpython_abi_hooks::propagate_native_failure(_py, "native descriptor");
                    DescriptorMutationOutcome::Error
                }
            }
        } else if object_type_id(ptr) == TYPE_ID_NATIVE_DESCRIPTOR {
            let value = match mutation {
                DescriptorMutation::Set(value) => Some(value),
                DescriptorMutation::Delete => None,
            };
            let result =
                crate::builtins::types::native_descriptor_mutate(_py, descriptor, instance, value);
            crate::call::discard_owned_call_result(_py, result);
            DescriptorMutationOutcome::Applied
        } else if object_type_id(ptr) == TYPE_ID_PROPERTY
            && crate::builtins::types::wrappers::wrapper_uses_default_method(
                _py,
                ptr,
                match mutation {
                    DescriptorMutation::Set(_) => b"__set__",
                    DescriptorMutation::Delete => b"__delete__",
                },
                match mutation {
                    DescriptorMutation::Set(_) => fn_key!(crate::molt_property_set),
                    DescriptorMutation::Delete => fn_key!(crate::molt_property_delete),
                },
            )
        {
            let value = match mutation {
                DescriptorMutation::Set(value) => Some(value),
                DescriptorMutation::Delete => None,
            };
            let result =
                crate::builtins::types::wrappers::property_mutate(_py, descriptor, instance, value);
            crate::call::discard_owned_call_result(_py, result);
            DescriptorMutationOutcome::Applied
        } else if exception_pending(_py) {
            DescriptorMutationOutcome::Error
        } else {
            let (hook, args) = match mutation {
                DescriptorMutation::Set(value) => (
                    DescriptorHook::Set,
                    DescriptorArguments::Two(instance, value),
                ),
                DescriptorMutation::Delete => {
                    (DescriptorHook::Delete, DescriptorArguments::One(instance))
                }
            };
            match call_descriptor_method(_py, descriptor, hook, args) {
                Some(result) => {
                    crate::call::discard_owned_call_result(_py, result);
                    DescriptorMutationOutcome::Applied
                }
                None if exception_pending(_py) => DescriptorMutationOutcome::Error,
                None => {
                    let is_data = descriptor_is_data(_py, descriptor);
                    if exception_pending(_py) {
                        DescriptorMutationOutcome::Error
                    } else if is_data {
                        let name = match mutation {
                            DescriptorMutation::Set(_) => "__set__",
                            DescriptorMutation::Delete => "__delete__",
                        };
                        raise_exception::<u64>(_py, "AttributeError", name);
                        DescriptorMutationOutcome::Error
                    } else {
                        DescriptorMutationOutcome::NotDescriptor
                    }
                }
            }
        };
        dec_ref_bits(_py, instance);
        dec_ref_bits(_py, descriptor);
        if exception_pending(_py) {
            DescriptorMutationOutcome::Error
        } else {
            outcome
        }
    }
}

pub(crate) unsafe fn class_attr_lookup(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    owner_ptr: *mut u8,
    instance_ptr: Option<*mut u8>,
    attr_bits: u64,
) -> Option<u64> {
    unsafe {
        crate::gil_assert();
        let val_bits = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)?;
        descriptor_bind(
            _py,
            val_bits,
            (!owner_ptr.is_null()).then(|| MoltObject::from_ptr(owner_ptr).bits()),
            instance_ptr.map(|ptr| MoltObject::from_ptr(ptr).bits()),
        )
    }
}

pub(crate) fn awaitable_await_func_bits(_py: &PyToken<'_>) -> u64 {
    builtin_func_bits(
        _py,
        NativeCallableSpec::function(&runtime_state(_py).special_cache.awaitable_await),
        fn_addr!(molt_awaitable_await),
        1,
    )
}

pub(crate) type SlotsInfo = crate::object::class_storage::ClassSlotPolicy;

/// Canonical admission policy for the two layout-owned instance attributes.
/// This must run before MRO descriptor binding: builtin roots may publish
/// class-level descriptors named `__dict__`/`__weakref__`, but those do not
/// create storage on exact slots-only instances.
pub(crate) unsafe fn class_instance_layout_attr_allowed(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    attr_bits: u64,
) -> Option<bool> {
    unsafe {
        let dict_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.dict_name, b"__dict__");
        if crate::object::ops_compare::string_storage_equal(attr_bits, dict_name_bits) {
            return Some(class_slots_info(_py, class_ptr).is_some_and(|info| info.allows_dict));
        }
        let weakref_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.weakref_name,
            b"__weakref__",
        );
        if crate::object::ops_compare::string_storage_equal(attr_bits, weakref_name_bits) {
            return Some(class_slots_info(_py, class_ptr).is_some_and(|info| info.allows_weakref));
        }
        None
    }
}

/// Hot queries project the admission record directly. Only unpublished class
/// construction can lack this record; mutable namespaces and MRO scans never
/// reclassify a published class's dictionary or weakref capabilities.
pub(crate) unsafe fn class_slots_info(_py: &PyToken<'_>, class_ptr: *mut u8) -> Option<SlotsInfo> {
    unsafe {
        crate::gil_assert();
        let policy = crate::object::layout::class_slot_policy(class_ptr);
        debug_assert!(
            policy.is_some() || !crate::object::class_definition_is_finished(class_ptr),
            "published definition requires admitted slot policy",
        );
        policy
    }
}

/// Prepare owned slot provenance independently of class allocation. Dynamic
/// type construction rejects malformed declarations before a finalizing
/// metaclass can observe a new class; static sealing consumes the same authority.
pub(crate) unsafe fn prepare_class_slot_declaration(
    py: &PyToken<'_>,
    dict_ptr: *mut u8,
    class_name: u64,
    bases: &[u64],
    native: Option<crate::object::class_storage::ClassSlotPolicy>,
) -> Option<u64> {
    unsafe {
        // Base choice precedes iteration, as in type.__new__. The admission
        // context pins every candidate across callbacks from the declaration.
        let admission =
            crate::object::class_layout::prepare_slot_admission(py, bases, native).ok()?;
        inc_ref_bits(py, MoltObject::from_ptr(dict_ptr).bits());
        let _namespace_owner = crate::PtrDropGuard::new(dict_ptr);
        let class_name = string_obj_to_owned(obj_from_bits(class_name)).unwrap_or_default();
        let private_prefix = class_name.trim_start_matches('_').as_bytes();
        let name = intern_static_name(py, &runtime_state(py).interned.slots_name, b"__slots__");
        if exception_pending(py) {
            return None;
        }
        let slots = dict_get_in_place(py, dict_ptr, name);
        if exception_pending(py) {
            return None;
        }
        if let Some(slots) = slots {
            inc_ref_bits(py, slots);
        }
        let _slots_owner = slots
            .and_then(|slots| obj_from_bits(slots).as_ptr())
            .map(crate::PtrDropGuard::new);
        let names = if let Some(slots) = slots {
            let names = if obj_from_bits(slots)
                .as_ptr()
                .is_some_and(|ptr| object_type_id(ptr) == TYPE_ID_STRING)
            {
                inc_ref_bits(py, slots);
                vec![slots]
            } else {
                crate::object::iterable::collect(
                    py,
                    slots,
                    crate::object::iterable::LengthHint::Consult,
                )?
            };
            Some(names)
        } else {
            None
        };
        // Keep all yielded owners alive before inspecting any item. An error
        // after a complete iteration must release every yielded value.
        let _source_owners: Vec<_> = names
            .as_ref()
            .into_iter()
            .flatten()
            .copied()
            .filter_map(|name| obj_from_bits(name).as_ptr().map(crate::PtrDropGuard::new))
            .collect();
        let policy = admission.validate(py, names.as_deref()).ok()?;
        let mut exact_names = Vec::new();
        let mut exact_owners = Vec::new();
        if let Some(names) = names {
            for name in names {
                let name = obj_from_bits(name).as_ptr().expect("validated slot string");
                let bytes = std::slice::from_raw_parts(string_bytes(name), string_len(name));
                let mut mangled = Vec::new();
                let bytes = if !private_prefix.is_empty()
                    && bytes.starts_with(b"__")
                    && !bytes.ends_with(b"__")
                    && !bytes.contains(&b'.')
                {
                    mangled.push(b'_');
                    mangled.extend_from_slice(private_prefix);
                    mangled.extend_from_slice(bytes);
                    &mangled
                } else {
                    bytes
                };
                // Namespace conflicts belong to preallocation admission. The
                // three construction metadata names are consumed by type setup.
                if bytes != b"__dict__"
                    && bytes != b"__weakref__"
                    && bytes != b"__qualname__"
                    && bytes != b"__classcell__"
                    && bytes != b"__classdictcell__"
                    && crate::object::ops::dict_get_str_bytes_borrowed(py, dict_ptr, bytes)
                        .is_some()
                {
                    let text = String::from_utf8_lossy(bytes);
                    raise_exception::<()>(
                        py,
                        "ValueError",
                        &format!("'{text}' in __slots__ conflicts with class variable"),
                    );
                    return None;
                }
                let exact = alloc_string(py, bytes);
                if exact.is_null() {
                    return None;
                }
                exact_owners.push(crate::PtrDropGuard::new(exact));
                exact_names.push(MoltObject::from_ptr(exact).bits());
            }
        }
        let names = if slots.is_some() {
            let tuple = alloc_tuple(py, &exact_names);
            if tuple.is_null() {
                return None;
            }
            exact_owners.push(crate::PtrDropGuard::new(tuple));
            MoltObject::from_ptr(tuple).bits()
        } else {
            MoltObject::none().bits()
        };
        let record = alloc_tuple(py, &[names, policy.encode()]);
        if record.is_null() {
            None
        } else {
            Some(MoltObject::from_ptr(record).bits())
        }
    }
}

/// Consume the declaration only once, including on retry after failed sealing.
unsafe fn capture_class_slot_declaration(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    dict_ptr: *mut u8,
) -> bool {
    use crate::object::layout::{
        ClassSlotDeclaration, class_set_slot_declaration_owned, class_slot_declaration,
    };
    unsafe {
        if !matches!(
            class_slot_declaration(class_ptr),
            ClassSlotDeclaration::Uninitialized
        ) {
            return true;
        }
        let bases = class_bases_vec(class_bases_bits(class_ptr));
        let native = crate::object::class_storage::class_native_slot_policy(class_ptr);
        let Some(declaration) = prepare_class_slot_declaration(
            _py,
            dict_ptr,
            class_name_bits(class_ptr),
            &bases,
            native,
        ) else {
            return false;
        };
        // A static constructor can expose callbacks while consuming its
        // declaration. Never publish policy for a graph changed by reentry.
        if !matches!(
            class_slot_declaration(class_ptr),
            ClassSlotDeclaration::Uninitialized
        ) || class_bases_vec(class_bases_bits(class_ptr)) != bases
        {
            dec_ref_bits(_py, declaration);
            raise_exception::<()>(
                _py,
                "RuntimeError",
                "class definition changed during slot admission",
            );
            return false;
        }
        class_set_slot_declaration_owned(class_ptr, declaration);
        true
    }
}

/// Capture immutable declaration provenance before deriving a solid owner or
/// sealing concrete storage. Builtin bootstrap may capture declarations before
/// the builtin table is published; this phase never queries that table.
pub(crate) unsafe fn capture_class_slot_declaration_for_seal(
    py: &PyToken<'_>,
    class: *mut u8,
) -> bool {
    unsafe {
        let Some(namespace) = obj_from_bits(class_dict_bits(class)).as_ptr() else {
            raise_exception::<()>(
                py,
                "SystemError",
                "class namespace is absent during slot capture",
            );
            return false;
        };
        if object_type_id(namespace) != TYPE_ID_DICT {
            raise_exception::<()>(py, "SystemError", "class namespace is not a dictionary");
            return false;
        }
        capture_class_slot_declaration(py, class, namespace)
    }
}

pub(crate) unsafe fn object_attr_lookup_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    unsafe { object_attr_lookup_with_policy(_py, obj_ptr, attr_bits, false) }
}

pub(crate) unsafe fn object_attr_lookup_with_policy(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    suppress: bool,
) -> Option<u64> {
    unsafe {
        object_attr_lookup_inner(
            _py,
            MoltObject::from_ptr(obj_ptr).bits(),
            obj_ptr,
            attr_bits,
            None,
            suppress,
        )
    }
}

/// Explicit C generic lookup replaces only the instance-dictionary tier. The
/// logical class and descriptor cache are the same authority used by ordinary
/// object.__getattribute__; physical C projection types never participate.
pub(crate) unsafe fn object_attr_lookup_with_dict(
    py: &PyToken<'_>,
    object: u64,
    name: u64,
    dictionary: u64,
    suppress: bool,
) -> Option<u64> {
    unsafe {
        let pointer = maybe_ptr_from_bits(object).unwrap_or(std::ptr::null_mut());
        if !pointer.is_null() && object_type_id(pointer) == TYPE_ID_DATACLASS {
            return dataclass_attr_lookup_inner(py, pointer, name, Some(dictionary), suppress);
        }
        object_attr_lookup_inner(py, object, pointer, name, Some(dictionary), suppress)
    }
}

// The semantic type belongs to type_of_bits, not the optional header class
// edge. Native tasks and other builtins can have a type without that edge.
unsafe fn object_attr_lookup_inner(
    _py: &PyToken<'_>,
    obj_bits: u64,
    obj_ptr: *mut u8,
    attr_bits: u64,
    dictionary: Option<u64>,
    suppress: bool,
) -> Option<u64> {
    unsafe {
        crate::gil_assert();
        let class_bits = type_of_bits(_py, obj_bits);
        let result = with_class_descriptor_snapshot(_py, class_bits, attr_bits, |snapshot| {
            if exception_pending(_py) {
                return None;
            }
            let class_ptr_opt = snapshot.and_then(|entry| obj_from_bits(entry.class_bits).as_ptr());
            if let Some(bits) = snapshot.and_then(|entry| entry.data_desc_bits) {
                let bound =
                    descriptor_bind(_py, bits, Some(type_of_bits(_py, obj_bits)), Some(obj_bits));
                if bound.is_some() || exception_pending(_py) {
                    return bound;
                }
            }
            // A logical class edge never grants access to inline words in a
            // fixed type object or a task's separately owned capture payload.
            let uses_inline_fields = dictionary.is_none()
                && !obj_ptr.is_null()
                && (crate::object::object_has_class_shape(obj_ptr)
                    || crate::object::native_instance::has_fields(obj_ptr));
            let mut field_offset_resolved = None;
            if let Some(class_ptr) = class_ptr_opt {
                // Field-offset IC: explicit dictionaries replace inferred storage.
                let attr_name_slice: Option<&[u8]> = obj_from_bits(attr_bits)
                    .as_ptr()
                    .filter(|&p| object_type_id(p) == TYPE_ID_STRING)
                    .map(|p| std::slice::from_raw_parts(string_bytes(p), string_len(p)));
                let current_type_version = crate::object::global_type_version();
                if uses_inline_fields
                    && let Some(name_bytes) = attr_name_slice
                    && let Some(offset) =
                        field_offset_ic_lookup(class_bits, name_bytes, current_type_version)
                {
                    profile_hit_unchecked(&FIELD_OFFSET_IC_HIT_COUNT);
                    field_offset_resolved = Some(offset);
                }
                if uses_inline_fields
                    && field_offset_resolved.is_none()
                    && let Some(offset) = class_inferred_field_offset(_py, class_ptr, attr_bits)
                {
                    profile_hit_unchecked(&FIELD_OFFSET_IC_MISS_COUNT);
                    field_offset_resolved = Some(offset);
                    if let Some(name_bytes) = attr_name_slice {
                        field_offset_ic_insert(
                            class_bits,
                            name_bytes,
                            current_type_version,
                            offset,
                        );
                    }
                }
            }
            let weakref_name_bits = intern_static_name(
                _py,
                &runtime_state(_py).interned.weakref_name,
                b"__weakref__",
            );
            if !obj_ptr.is_null()
                && crate::object::ops_compare::string_storage_equal(attr_bits, weakref_name_bits)
            {
                if let Some(class_ptr) = class_ptr_opt
                    && class_instance_layout_attr_allowed(_py, class_ptr, attr_bits) == Some(false)
                {
                    return None;
                }
                return Some(crate::object::weakref::weakref_head_for_target(
                    _py, obj_bits,
                ));
            }
            let instance = if let Some(dictionary) = dictionary {
                crate::object::accessors::attribute_dictionary_lookup(
                    _py, dictionary, attr_bits, suppress,
                )
            } else if !obj_ptr.is_null() {
                crate::object::accessors::instance_attribute_lookup_with_policy(
                    _py,
                    obj_ptr,
                    attr_bits,
                    field_offset_resolved,
                    suppress,
                )
            } else {
                None
            };
            if instance.is_some() || exception_pending(_py) {
                return instance;
            }
            // Dictionary callbacks may change both the class namespace and
            // __class__. Keep the selected value, but bind to the live owner.
            snapshot
                .and_then(|entry| entry.class_attr_bits)
                .and_then(|bits| {
                    descriptor_bind(_py, bits, Some(type_of_bits(_py, obj_bits)), Some(obj_bits))
                })
        });
        if result.is_some() || exception_pending(_py) {
            return result;
        }
        // Internal futures expose their poll adapter only after normal class
        // and dictionary lookup misses. Absence of an attached class edge is
        // a storage fact here, never evidence that the object has no type.
        if crate::async_rt::generators::is_native_poll_future_bits(obj_bits) {
            let await_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.await_name, b"__await__");
            if crate::object::ops_compare::string_storage_equal(attr_bits, await_name_bits) {
                let func_bits = awaitable_await_func_bits(_py);
                return Some(molt_bound_method_new(func_bits, obj_bits));
            }
        }
        None
    }
}

/// Resolution outcome for the per-site method inline cache.
pub(crate) struct MethodIcResolution {
    pub(crate) class_bits: u64,
    pub(crate) class_version: u64,
    pub(crate) func_bits: u64,
    /// Whether an instance of this class could carry an OWN attribute of this
    /// name (a managed field slot).  When false, instance-shadow checks may be
    /// skipped on IC hits — a non-data class method can never be shadowed.
    pub(crate) can_shadow: bool,
}

/// Class-side resolution for the fused method fast path (everything except the
/// per-instance shadow check). Calls `consume` while the selected class and
/// plain-function method are pinned. Resolution metadata is borrowed only for
/// that callback; it must retain any function ownership that escapes.
/// Returns `None` for shapes outside this path (non-class type, custom
/// `__getattribute__`, data descriptor, or non-function attribute).
///
/// # Safety
/// `obj_ptr` must be live; the GIL must be held.
pub(crate) unsafe fn object_method_ic_resolve<R>(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    consume: impl FnOnce(MethodIcResolution) -> R,
) -> Option<R> {
    unsafe {
        crate::gil_assert();
        let type_id = object_type_id(obj_ptr);
        if !crate::object::object_has_class_shape(obj_ptr) && type_id != TYPE_ID_DATACLASS {
            return None;
        }
        let class_bits = object_class_bits(obj_ptr);
        if class_bits == 0 {
            return None;
        }
        let class_ptr = obj_from_bits(class_bits).as_ptr()?;
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            return None;
        }

        // (2) Bail out if the class installs a custom __getattribute__ — its
        // observable behaviour must run.  A custom __getattr__ only fires on
        // AttributeError (i.e. a FAILED lookup), so it cannot change the result
        // of a SUCCESSFUL method resolution and is intentionally not checked.
        let getattribute_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.getattribute_name,
            b"__getattribute__",
        );
        let getattribute_raw = class_attr_lookup_raw_mro(_py, class_ptr, getattribute_bits);
        if let Some(raw_bits) = getattribute_raw {
            let default_bits =
                crate::builtins::methods::object_method_bits(_py, "__getattribute__")?;
            if raw_bits != default_bits {
                return None;
            }
        }

        // Use the same initial MRO snapshot as ordinary lookup. The consumer
        // acquires its call owner before this snapshot can retire or run code.
        let class_version = class_layout_version_bits(class_ptr);
        with_class_descriptor_snapshot(_py, class_bits, attr_bits, |snapshot| {
            let snapshot = snapshot?;
            if snapshot.data_desc_bits.is_some() {
                return None;
            }
            let class_attr_bits = snapshot.class_attr_bits?;

            // (3 cont.) Only a plain function qualifies for the unbound fast path.
            let func_ptr = maybe_ptr_from_bits(class_attr_bits)?;
            if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
                return None;
            }

            let receiver = MoltObject::from_ptr(obj_ptr).bits();
            if !matches!(
                function_descriptor_receiver(_py, func_ptr, Some(class_bits), Some(receiver)),
                Ok(Some(admitted)) if admitted.bits() == receiver
            ) {
                return None;
            }

            // `can_shadow` is a CLASS-level property (the IC is keyed on the class,
            // not the instance, so it must hold for EVERY instance of the class).
            // A non-data class method is shadowed only by an instance OWN attribute,
            // which requires either a managed field slot for the name OR a dynamic
            // instance `__dict__`.  We only prove "cannot shadow" when the class
            // layout has no offset for `attr` AND the class forbids an instance
            // `__dict__` (slots-only without `__dict__`).  Otherwise stay
            // conservative (`true`) and keep the cheap per-call shadow check.
            let has_field_offset = class_inferred_field_offset(_py, class_ptr, attr_bits).is_some();
            // Incomplete construction cannot install an attribute-cache proof.
            let allows_instance_dict = class_slots_info(_py, class_ptr)?.allows_dict;
            let can_shadow = has_field_offset || allows_instance_dict;

            Some(consume(MethodIcResolution {
                class_bits,
                class_version,
                func_bits: class_attr_bits,
                can_shadow,
            }))
        })
    }
}

/// Public wrapper of [`instance_shadows_attr`] for the per-site method IC's
/// hit-time validation.
///
/// # Safety
/// `obj_ptr`/`class_ptr` must be live; the GIL must be held.
pub(crate) unsafe fn object_instance_shadows(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    class_ptr: *mut u8,
    attr_bits: u64,
) -> bool {
    unsafe { instance_shadows_attr(_py, obj_ptr, class_ptr, attr_bits) }
}

/// True when `obj` has an OWN attribute named `attr_bits` (an instance field
/// slot holding a present value, or a `__dict__` entry).  Mirrors the
/// instance-precedence portion of `object_attr_lookup_raw`.
///
/// # Safety
/// Pointers must be live; the GIL must be held.
unsafe fn instance_shadows_attr(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    class_ptr: *mut u8,
    attr_bits: u64,
) -> bool {
    unsafe {
        let offset = class_inferred_field_offset(_py, class_ptr, attr_bits);
        if let Some(bits) =
            crate::object::accessors::instance_attribute_lookup(_py, obj_ptr, attr_bits, offset)
        {
            dec_ref_bits(_py, bits);
            return true;
        }
        false
    }
}

/// Resolution outcome for the per-site super-method inline cache.
pub(crate) struct SuperIcResolution {
    /// `type(self)` — the IC key class (super resolution depends on the runtime
    /// type of the instance, not the defining class alone).
    pub(crate) self_class_bits: u64,
    pub(crate) self_class_version: u64,
    pub(crate) func_bits: u64,
}

/// One raw super selection over the receiver MRO strictly after the start
/// class. The selected namespace value is retained before the MRO pin retires.
/// Unlike ordinary class lookup this never restarts from a declaring class.
unsafe fn super_attribute_owned(
    py: &PyToken<'_>,
    start: u64,
    receiver_class: u64,
    name: u64,
) -> Option<(u64, u64)> {
    unsafe {
        let class = obj_from_bits(receiver_class).as_ptr()?;
        if object_type_id(class) != TYPE_ID_TYPE {
            return None;
        }
        let mro = class_mro_view(py, class);
        let mut after_start = false;
        for declaring in mro.iter().copied() {
            if !after_start {
                after_start = declaring == start;
                continue;
            }
            let Some(owner) = obj_from_bits(declaring).as_ptr() else {
                continue;
            };
            if object_type_id(owner) != TYPE_ID_TYPE {
                continue;
            }
            if let Some(value) = class_namespace_lookup_raw(py, owner, name) {
                inc_ref_bits(py, value);
                return Some((declaring, value));
            }
            if exception_pending(py) {
                return None;
            }
        }
        None
    }
}

/// Normal super lookup delegates first; a clean miss and __class__ inspect the
/// proxy itself. Explicit object/C-generic lookup never enters this protocol.
pub(crate) unsafe fn super_attr_lookup(py: &PyToken<'_>, proxy: *mut u8, name: u64) -> Option<u64> {
    unsafe {
        let spelling = string_obj_to_owned(obj_from_bits(name));
        let receiver = crate::super_obj_bits(proxy);
        let receiver_class = crate::object::layout::super_receiver_class_bits(proxy);
        if spelling.as_deref() != Some("__class__")
            && let Some((_, selected)) =
                super_attribute_owned(py, crate::super_type_bits(proxy), receiver_class, name)
        {
            let instance = (receiver != receiver_class).then_some(receiver);
            let bound = descriptor_bind(py, selected, Some(receiver_class), instance);
            dec_ref_bits(py, selected);
            return bound;
        }
        if exception_pending(py) {
            return None;
        }
        object_attr_lookup_with_policy(py, proxy, name, false)
    }
}

/// Optimized super calls consume the same MRO-suffix selection as materialized
/// proxies. Only plain Python instance methods qualify. The returned function is OWNED
/// until the call-site cache pins it or rejects the selection.
///
/// # Safety
/// All inputs must be live and the GIL held.
pub(crate) unsafe fn super_resolve_method_unbound(
    py: &PyToken<'_>,
    start_class_bits: u64,
    self_bits: u64,
    attr_bits: u64,
) -> Option<SuperIcResolution> {
    unsafe {
        crate::gil_assert();
        let self_ptr = maybe_ptr_from_bits(self_bits)?;
        if object_type_id(self_ptr) == TYPE_ID_TYPE
            || string_obj_to_owned(obj_from_bits(attr_bits)).as_deref() == Some("__class__")
        {
            return None;
        }
        let obj_type_bits = type_of_bits(py, self_bits);
        let obj_type_ptr = obj_from_bits(obj_type_bits).as_ptr()?;
        if object_type_id(obj_type_ptr) != TYPE_ID_TYPE {
            return None;
        }
        let self_class_version = class_layout_version_bits(obj_type_ptr);
        let (_, value) = super_attribute_owned(py, start_class_bits, obj_type_bits, attr_bits)?;
        let eligible = obj_from_bits(value).as_ptr().is_some_and(|ptr| {
            object_type_id(ptr) == TYPE_ID_FUNCTION
                && crate::builtins::functions::native_callable::NativeCallableKind::from_class(
                    py,
                    object_class_bits(ptr),
                )
                .is_none()
                && matches!(
                    function_descriptor_receiver(py, ptr, Some(obj_type_bits), Some(self_bits)),
                    Ok(Some(admitted)) if admitted.bits() == self_bits
                )
        });
        if !eligible {
            dec_ref_bits(py, value);
            return None;
        }
        Some(SuperIcResolution {
            self_class_bits: obj_type_bits,
            self_class_version,
            func_bits: value,
        })
    }
}

#[cfg(test)]
pub(crate) unsafe fn dataclass_attr_lookup_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    unsafe { dataclass_attr_lookup_inner(_py, obj_ptr, attr_bits, None, false) }
}

pub(crate) unsafe fn dataclass_attr_lookup_inner(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
    dictionary: Option<u64>,
    suppress: bool,
) -> Option<u64> {
    unsafe {
        crate::gil_assert();
        let obj_bits = MoltObject::from_ptr(obj_ptr).bits();
        let desc_ptr = dataclass_desc_ptr(obj_ptr);
        if desc_ptr.is_null() {
            return None;
        }
        let attr_name = string_obj_to_owned(obj_from_bits(attr_bits));
        let class_bits = object_class_bits(obj_ptr);
        let offset = attr_name
            .as_deref()
            .and_then(|name| (*desc_ptr).field_name_to_index.get(name).copied())
            .map(|index| index * std::mem::size_of::<u64>());
        if let Some(offset) = offset
            && crate::object::field_storage::field_at_offset(_py, obj_ptr, offset)
                .is_some_and(|field| field.kind.is_declared_slot())
        {
            return crate::object::accessors::instance_attribute_lookup(
                _py,
                obj_ptr,
                attr_bits,
                Some(offset),
            );
        }
        with_class_descriptor_snapshot(_py, class_bits, attr_bits, |snapshot| {
            if exception_pending(_py) {
                return None;
            }
            if let Some(bits) = snapshot.and_then(|entry| entry.data_desc_bits) {
                let bound =
                    descriptor_bind(_py, bits, Some(type_of_bits(_py, obj_bits)), Some(obj_bits));
                if bound.is_some() || exception_pending(_py) {
                    return bound;
                }
            }
            let instance = if let Some(dictionary) = dictionary {
                crate::object::accessors::attribute_dictionary_lookup(
                    _py, dictionary, attr_bits, suppress,
                )
            } else {
                crate::object::accessors::instance_attribute_lookup_with_policy(
                    _py, obj_ptr, attr_bits, offset, suppress,
                )
            };
            if instance.is_some() || exception_pending(_py) {
                return instance;
            }
            snapshot
                .and_then(|entry| entry.class_attr_bits)
                .and_then(|bits| {
                    descriptor_bind(_py, bits, Some(type_of_bits(_py, obj_bits)), Some(obj_bits))
                })
        })
    }
}
