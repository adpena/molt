use crate::PyToken;
use std::cell::RefCell;
use std::collections::HashSet;
use std::sync::OnceLock;

use molt_obj_model::{ExceptionTypedField, MoltObject};

use crate::builtins::annotations::pep649_enabled;
use crate::builtins::exceptions::{
    exception_matches_builtin_name, exception_typed_fields_replace_internal,
    molt_exception_last_pending,
};
use crate::{
    ClassEdgeOwnership, FIELD_OFFSET_IC_HIT_COUNT, FIELD_OFFSET_IC_MISS_COUNT, TYPE_ID_CALL_ITER,
    TYPE_ID_CLASSMETHOD, TYPE_ID_DATACLASS, TYPE_ID_DICT, TYPE_ID_ENUMERATE, TYPE_ID_EXCEPTION,
    TYPE_ID_FILE_HANDLE, TYPE_ID_FILTER, TYPE_ID_FUNCTION, TYPE_ID_GENERATOR, TYPE_ID_GLOB_ITER,
    TYPE_ID_ITER, TYPE_ID_MAP, TYPE_ID_MODULE, TYPE_ID_NATIVE_DESCRIPTOR, TYPE_ID_PROPERTY,
    TYPE_ID_REVERSED, TYPE_ID_STATICMETHOD, TYPE_ID_STRING, TYPE_ID_TYPE, TYPE_ID_ZIP,
    alloc_dict_with_pairs, alloc_function_obj, alloc_property_obj, alloc_string,
    builtin_class_method_bits, builtin_classes, builtin_func_bits, call_callable1, call_callable2,
    call_callable3, class_bases_bits, class_bases_vec, class_dict_bits, class_layout_version_bits,
    class_mro_pinned, class_mro_vec, class_mro_view, class_name_bits, clear_exception,
    dataclass_desc_ptr, dec_ref_bits, dict_get_in_place, dict_order, dict_set_in_place,
    exception_dict_bits, exception_last_bits_noinc, exception_pending, exception_stack_pop,
    exception_stack_push, inc_ref_bits, init_atomic_bits, instance_dict_bits, intern_static_name,
    is_builtin_class_bits, is_missing_bits, is_truthy, maybe_ptr_from_bits, module_dict_bits,
    molt_awaitable_await, molt_bound_method_new, molt_function_get_code, molt_function_get_globals,
    obj_eq, obj_from_bits, object_class_bits, object_field_get_ptr_raw, object_type_id,
    profile_hit_unchecked, raise_exception, runtime_state, string_bytes, string_len,
    string_obj_to_owned, type_name, type_of_bits,
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
                    assert!(super::apply_class_slots_layout(_py, ptr));
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
                    assert!(super::apply_class_slots_layout(_py, ptr));
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
            assert!(unsafe { super::class_slots_info(_py, ptr) }.is_none());
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

            let rebound_ptr =
                crate::alloc_dict_with_pairs(_py, &[field, MoltObject::from_int(8).bits()]);
            let rebound = MoltObject::from_ptr(rebound_ptr).bits();
            crate::molt_set_attr_name(class, offsets_name, rebound);
            assert!(crate::exception_pending(_py));
            crate::molt_exception_clear();
            assert_eq!(
                unsafe { super::class_field_offset(_py, class_ptr, field) },
                Some(0)
            );
            crate::molt_del_attr_name(class, offsets_name);
            assert!(crate::exception_pending(_py));
            crate::molt_exception_clear();
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
            assert_eq!(edges.iter().filter(|&&bits| bits == offsets).count(), 1);
            assert_eq!(heap_refcount(offsets), 2, "local plus private TYPE owner");
            unsafe { crate::object::heap_lifecycle::clear_cycle_edges(_py, class_ptr) };
            assert_eq!(
                heap_refcount(offsets),
                1,
                "cycle teardown detaches private owner once"
            );

            for bits in [
                name,
                field,
                offsets_name,
                size_name,
                offsets,
                rebound,
                class,
            ] {
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
            let name_bits =
                crate::builtins::exceptions::exception_typed_field_get(_py, exc_ptr, "name")
                    .expect("AttributeError.name descriptor")
                    .expect("AttributeError.name value");
            let member_obj_bits =
                crate::builtins::exceptions::exception_typed_field_get(_py, exc_ptr, "obj")
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
            cached.release(_py);
            assert_eq!(heap_refcount(class_bits), class_before + 1);
            assert_eq!(heap_refcount(first_bits), first_before + 1);

            descriptor_cache_store(_py, class_bits, attr_bits, 2, None, Some(second_bits));
            assert_eq!(heap_refcount(class_bits), class_before + 1);
            assert_eq!(heap_refcount(first_bits), first_before);
            assert_eq!(heap_refcount(second_bits), second_before + 1);
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

fn debug_class_layout_filter() -> Option<&'static str> {
    static FILTER: OnceLock<Option<String>> = OnceLock::new();
    FILTER
        .get_or_init(|| {
            std::env::var("MOLT_DEBUG_CLASS_LAYOUT")
                .ok()
                .map(|raw| raw.trim().to_string())
                .filter(|val| !val.is_empty())
        })
        .as_deref()
}

fn debug_class_layout_match(class_name: &str) -> bool {
    match debug_class_layout_filter() {
        Some("1") => true,
        Some(filter) => class_name.contains(filter),
        None => false,
    }
}

pub(crate) struct DescriptorCacheEntry {
    pub(crate) class_bits: u64,
    pub(crate) attr_name: Vec<u8>,
    pub(crate) version: u64,
    pub(crate) data_desc_bits: Option<u64>,
    pub(crate) class_attr_bits: Option<u64>,
}

impl DescriptorCacheEntry {
    fn retain_from_entry(_py: &PyToken<'_>, entry: &Self) -> Self {
        Self::retain(
            _py,
            entry.class_bits,
            entry.attr_name.clone(),
            entry.version,
            entry.data_desc_bits,
            entry.class_attr_bits,
        )
    }

    fn retain(
        _py: &PyToken<'_>,
        class_bits: u64,
        attr_name: Vec<u8>,
        version: u64,
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
            attr_name,
            version,
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
// On hit we skip the full MRO walk performed by `class_field_offset` and go
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
    /// resolution via the slow `class_field_offset` path.
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
    Optional,
}

unsafe fn module_attr_lookup_impl(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    attr_bits: u64,
    policy: ModuleLookupPolicy,
) -> Option<u64> {
    unsafe {
        crate::gil_assert();
        let class_name =
            intern_static_name(_py, &runtime_state(_py).interned.class_name, b"__class__");
        if obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(class_name)) {
            let class = type_of_bits(_py, MoltObject::from_ptr(ptr).bits());
            inc_ref_bits(_py, class);
            return Some(class);
        }
        let dict_bits = module_dict_bits(ptr);
        let dict_obj = obj_from_bits(dict_bits);
        let dict_ptr = dict_obj.as_ptr()?;
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return None;
        }
        let dict_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.dict_name, b"__dict__");
        if obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(dict_name_bits)) {
            inc_ref_bits(_py, dict_bits);
            return Some(dict_bits);
        }
        let annotations_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.annotations_name,
            b"__annotations__",
        );
        if obj_eq(
            _py,
            obj_from_bits(attr_bits),
            obj_from_bits(annotations_name_bits),
        ) {
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
        if obj_eq(
            _py,
            obj_from_bits(attr_bits),
            obj_from_bits(annotate_name_bits),
        ) {
            if let Some(val_bits) = dict_get_in_place(_py, dict_ptr, annotate_name_bits) {
                inc_ref_bits(_py, val_bits);
                return Some(val_bits);
            }
            let none_bits = MoltObject::none().bits();
            inc_ref_bits(_py, none_bits);
            return Some(none_bits);
        }
        if let Some(val) = dict_get_in_place(_py, dict_ptr, attr_bits) {
            inc_ref_bits(_py, val);
            return Some(val);
        }
        if policy == ModuleLookupPolicy::Default {
            return None;
        }
        let getattr_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.getattr_name,
            b"__getattr__",
        );
        if !obj_eq(
            _py,
            obj_from_bits(attr_bits),
            obj_from_bits(getattr_name_bits),
        ) && let Some(getattr_bits) = dict_get_in_place(_py, dict_ptr, getattr_name_bits)
        {
            let allow_missing = policy == ModuleLookupPolicy::Optional;
            if allow_missing {
                exception_stack_push();
            }
            inc_ref_bits(_py, getattr_bits);
            let res_bits = call_callable1(_py, getattr_bits, attr_bits);
            dec_ref_bits(_py, getattr_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, res_bits);
                if allow_missing && clear_attribute_error_if_pending(_py) {
                    exception_stack_pop(_py);
                    return None;
                }
                if allow_missing {
                    exception_stack_pop(_py);
                }
                return None;
            }
            if allow_missing {
                exception_stack_pop(_py);
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

pub(crate) unsafe fn module_attr_lookup_allow_missing(
    _py: &PyToken<'_>,
    ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    unsafe { module_attr_lookup_impl(_py, ptr, attr_bits, ModuleLookupPolicy::Optional) }
}

pub(crate) unsafe fn module_attr_lookup_default(
    py: &PyToken<'_>,
    ptr: *mut u8,
    name: u64,
) -> Option<u64> {
    unsafe { module_attr_lookup_impl(py, ptr, name, ModuleLookupPolicy::Default) }
}

pub(crate) unsafe fn dir_collect_from_dict_ptr(
    dict_ptr: *mut u8,
    seen: &mut HashSet<String>,
    out: &mut Vec<u64>,
) {
    unsafe {
        crate::gil_assert();
        let order = dict_order(dict_ptr);
        for pair in order.chunks_exact(2) {
            let key_bits = pair[0];
            if let Some(name) = string_obj_to_owned(obj_from_bits(key_bits))
                && seen.insert(name)
            {
                out.push(key_bits);
            }
        }
    }
}

pub(crate) unsafe fn dir_collect_from_class_bits(
    class_bits: u64,
    seen: &mut HashSet<String>,
    out: &mut Vec<u64>,
) {
    unsafe {
        crate::gil_assert();
        for base_bits in class_mro_vec(class_bits) {
            let class_obj = obj_from_bits(base_bits);
            let Some(class_ptr) = class_obj.as_ptr() else {
                continue;
            };
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                continue;
            }
            let dict_bits = class_dict_bits(class_ptr);
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                continue;
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                continue;
            }
            dir_collect_from_dict_ptr(dict_ptr, seen, out);
        }
    }
}

pub(crate) unsafe fn dir_collect_from_instance(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    seen: &mut HashSet<String>,
    out: &mut Vec<u64>,
) {
    unsafe {
        crate::gil_assert();
        // CPython's `dir()` includes instance `__dict__` keys, but it must not call
        // `getattr(obj, "__dict__")` (which can run arbitrary user code and/or recurse).
        //
        // Instead, consult the runtime's internal dict storage for the handful of object
        // categories that actually have one.
        let dict_bits = match object_type_id(obj_ptr) {
            type_id if crate::object::heap_kind_has_class_shape(type_id) => {
                instance_dict_bits(obj_ptr)
            }
            TYPE_ID_DATACLASS => instance_dict_bits(obj_ptr),
            TYPE_ID_EXCEPTION => exception_dict_bits(obj_ptr),
            TYPE_ID_MODULE => module_dict_bits(obj_ptr),
            _ => 0,
        };
        if dict_bits == 0 || obj_from_bits(dict_bits).is_none() {
            return;
        }
        let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
            return;
        };
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return;
        }
        dir_collect_from_dict_ptr(dict_ptr, seen, out);
    }
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

/// Return a borrowed class-dictionary/cache value. Any consumer retaining it
/// across user code must own a reference; `descriptor_bind` and
/// `descriptor_call1` establish that ownership for their immediate operation.
pub(crate) unsafe fn class_attr_lookup_raw_mro(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    unsafe {
        crate::gil_assert();
        let attr_name = string_obj_to_owned(obj_from_bits(attr_bits));
        if let Some(name) = attr_name.as_deref()
            && (name == "__code__" || name == "__globals__")
        {
            let builtins = builtin_classes(_py);
            let class_bits = MoltObject::from_ptr(class_ptr).bits();
            if class_bits == builtins.function {
                let bits = if name == "__code__" {
                    function_code_descriptor_bits(_py)
                } else {
                    function_globals_descriptor_bits(_py)
                };
                if bits != 0 {
                    return Some(bits);
                }
            }
        }
        let debug_bound = crate::builtins::attributes::debug_bound_method_enabled();
        if let Some(mro) = class_mro_pinned(_py, class_ptr) {
            for class_bits in mro.iter() {
                let class_obj = obj_from_bits(*class_bits);
                let Some(ptr) = class_obj.as_ptr() else {
                    continue;
                };
                if object_type_id(ptr) != TYPE_ID_TYPE {
                    continue;
                }
                let dict_bits = class_dict_bits(ptr);
                let dict_obj = obj_from_bits(dict_bits);
                let Some(dict_ptr) = dict_obj.as_ptr() else {
                    continue;
                };
                if object_type_id(dict_ptr) != TYPE_ID_DICT {
                    continue;
                }
                if let Some(val_bits) = dict_get_in_place(_py, dict_ptr, attr_bits) {
                    if debug_bound && let Some(name) = attr_name.as_deref() {
                        let class_name_bits = class_name_bits(ptr);
                        let class_name = string_obj_to_owned(obj_from_bits(class_name_bits))
                            .unwrap_or_else(|| "<unknown>".to_string());
                        let val_obj = obj_from_bits(val_bits);
                        let (val_type_id, val_type_name) = match val_obj.as_ptr() {
                            Some(val_ptr) => (
                                object_type_id(val_ptr),
                                type_name(_py, val_obj).into_owned(),
                            ),
                            None => (0, format!("immediate:{:#x}", val_bits)),
                        };
                        if class_name == "ThreadPoolExecutor" || class_name == "Executor" {
                            eprintln!(
                                "class_attr_lookup_raw_mro: attr={} class={} val_bits={:#x} val_type_id={} val_type={}",
                                name, class_name, val_bits, val_type_id, val_type_name
                            );
                        }
                    }
                    return Some(val_bits);
                }
                // Clear any exception left by the failed dict lookup
                clear_attribute_error_if_pending(_py);
                if let Some(name) = attr_name.as_deref()
                    && (is_builtin_class_bits(_py, *class_bits)
                        || crate::builtins::exceptions::is_builtin_exception_class_bits(
                            _py,
                            *class_bits,
                        ))
                    && let Some(func_bits) = builtin_class_method_bits(_py, *class_bits, name)
                {
                    return Some(func_bits);
                }
            }
            // __doc__ defaults to None for all classes (CPython parity).
            // Builtin types and types.ModuleType don't store __doc__ in their
            // class dict, but cls.__doc__ must still return None, not raise.
            if attr_name.as_deref() == Some("__doc__") {
                return Some(MoltObject::none().bits());
            }
            return None;
        }
        let mut current_ptr = class_ptr;
        let mut depth = 0usize;
        loop {
            let dict_bits = class_dict_bits(current_ptr);
            let dict_obj = obj_from_bits(dict_bits);
            let dict_ptr = dict_obj.as_ptr()?;
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                return None;
            }
            if let Some(val_bits) = dict_get_in_place(_py, dict_ptr, attr_bits) {
                return Some(val_bits);
            }
            // Clear any exception left by the failed dict lookup
            clear_attribute_error_if_pending(_py);
            if let Some(name) = attr_name.as_deref() {
                let current_bits = MoltObject::from_ptr(current_ptr).bits();
                if (is_builtin_class_bits(_py, current_bits)
                    || crate::builtins::exceptions::is_builtin_exception_class_bits(
                        _py,
                        current_bits,
                    ))
                    && let Some(func_bits) = builtin_class_method_bits(_py, current_bits, name)
                {
                    return Some(func_bits);
                }
            }
            let bases_bits = class_bases_bits(current_ptr);
            let bases = class_bases_vec(bases_bits);
            let next_bits = bases.first().copied()?;
            let next_obj = obj_from_bits(next_bits);
            let next_ptr = next_obj.as_ptr()?;
            if object_type_id(next_ptr) != TYPE_ID_TYPE {
                return None;
            }
            if next_ptr == current_ptr {
                return None;
            }
            current_ptr = next_ptr;
            depth += 1;
            if depth > 64 {
                return None;
            }
        }
    }
}

/// Resolve the physical map authority for one class. Finished classes use the
/// retained TYPE payload edge exclusively; only an unfinished construction may
/// consult its mutable namespace.
pub(crate) unsafe fn class_field_offsets_map_bits(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    construction_name_bits: Option<u64>,
) -> Option<u64> {
    unsafe {
        if crate::object::class_definition_is_finished(class_ptr) {
            let bits = crate::object::layout::class_field_offsets_bits(class_ptr);
            assert_ne!(
                bits, 0,
                "sealed class has no private field-offset authority"
            );
            return (!obj_from_bits(bits).is_none()).then_some(bits);
        }
        let dict_ptr = obj_from_bits(class_dict_bits(class_ptr)).as_ptr()?;
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return None;
        }
        let fields_name = construction_name_bits.unwrap_or_else(|| {
            intern_static_name(
                _py,
                &runtime_state(_py).interned.field_offsets_name,
                b"__molt_field_offsets__",
            )
        });
        let bits = dict_get_in_place(_py, dict_ptr, fields_name)?;
        (!obj_from_bits(bits).is_none()).then_some(bits)
    }
}

/// Compare exact runtime strings without hashing or invoking Python equality.
/// Sealed physical-layout consumers use this after validating both identities.
pub(crate) unsafe fn exact_string_bits_equal(left_bits: u64, right_bits: u64) -> bool {
    unsafe {
        let Some(left) = obj_from_bits(left_bits).as_ptr() else {
            return false;
        };
        let Some(right) = obj_from_bits(right_bits).as_ptr() else {
            return false;
        };
        if object_type_id(left) != TYPE_ID_STRING || object_type_id(right) != TYPE_ID_STRING {
            return false;
        }
        let left_len = string_len(left);
        let right_len = string_len(right);
        left_len == right_len
            && std::slice::from_raw_parts(string_bytes(left), left_len)
                == std::slice::from_raw_parts(string_bytes(right), right_len)
    }
}

/// Exact-string lookup over dict insertion order. Sealed maps have already
/// validated every key/value pair, so this performs neither hashing nor Python
/// equality and is safe for GC-adjacent physical-layout consumers.
unsafe fn field_offset_in_map(offsets_bits: u64, attr_bits: u64) -> Option<usize> {
    unsafe {
        let attr_ptr = obj_from_bits(attr_bits).as_ptr()?;
        if object_type_id(attr_ptr) != TYPE_ID_STRING {
            return None;
        }
        let offsets_ptr = obj_from_bits(offsets_bits).as_ptr()?;
        if object_type_id(offsets_ptr) != TYPE_ID_DICT {
            return None;
        }
        for pair in dict_order(offsets_ptr).chunks_exact(2) {
            if exact_string_bits_equal(pair[0], attr_bits) {
                return obj_from_bits(pair[1])
                    .as_int()
                    .and_then(|offset| usize::try_from(offset).ok());
            }
        }
        None
    }
}

pub(crate) unsafe fn class_field_offset(
    _py: &PyToken<'_>,
    class_ptr: *mut u8,
    attr_bits: u64,
) -> Option<usize> {
    unsafe {
        crate::gil_assert();
        let mro = class_mro_view(_py, class_ptr);
        for class_bits in mro.iter().copied() {
            let Some(current_ptr) = obj_from_bits(class_bits).as_ptr() else {
                continue;
            };
            if object_type_id(current_ptr) != TYPE_ID_TYPE {
                continue;
            }
            let Some(offsets_bits) = class_field_offsets_map_bits(_py, current_ptr, None) else {
                continue;
            };
            if let Some(offset) = field_offset_in_map(offsets_bits, attr_bits) {
                return Some(offset);
            }
        }
        None
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
                .any(|name| exact_string_bits_equal(name, attr_bits))
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
        let offsets_bits = class_field_offsets_map_bits(_py, class_ptr, None)?;
        field_offset_in_map(offsets_bits, attr_bits)
    }
}

/// Probe a type slot without invoking its descriptor. Protocol admission must
/// not bind a method that is only observed when the operation executes.
pub(crate) unsafe fn has_special_method(py: &PyToken<'_>, bits: u64, name: &[u8]) -> bool {
    unsafe {
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
        let instance = obj_from_bits(bits).as_ptr()?;
        let class = obj_from_bits(type_of_bits(py, bits)).as_ptr()?;
        let name = attr_name_bits_from_bytes(py, name)?;
        let method = class_attr_lookup(py, class, class, Some(instance), name);
        dec_ref_bits(py, name);
        if exception_pending(py) {
            if let Some(method) = method {
                dec_ref_bits(py, method);
            }
            return None;
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
) -> Option<DescriptorCacheEntry> {
    crate::gil_assert();
    let attr_name = string_obj_to_owned(obj_from_bits(attr_bits))?;
    let attr_bytes = attr_name.as_bytes();
    DESCRIPTOR_CACHE_TLS.with(|cell| {
        cell.borrow()
            .as_ref()
            .filter(|entry| {
                entry.class_bits == class_bits
                    && entry.version == version
                    && entry.attr_name == attr_bytes
            })
            .map(|entry| DescriptorCacheEntry::retain_from_entry(_py, entry))
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
    let Some(attr_name) = string_obj_to_owned(obj_from_bits(attr_bits)) else {
        return;
    };
    let entry = DescriptorCacheEntry::retain(
        _py,
        class_bits,
        attr_name.into_bytes(),
        version,
        data_desc_bits,
        class_attr_bits,
    );
    let old_entry = DESCRIPTOR_CACHE_TLS.with(|cell| cell.borrow_mut().replace(entry));
    if let Some(old_entry) = old_entry {
        old_entry.release(_py);
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

pub(crate) unsafe fn descriptor_is_data(_py: &PyToken<'_>, val_bits: u64) -> bool {
    unsafe {
        crate::gil_assert();
        let Some(val_ptr) = maybe_ptr_from_bits(val_bits) else {
            return false;
        };
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

pub(crate) unsafe fn attr_lookup_ptr_any(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    unsafe {
        crate::gil_assert();
        match object_type_id(obj_ptr) {
            type_id if crate::object::heap_kind_has_class_shape(type_id) => {
                object_attr_lookup_raw(_py, obj_ptr, attr_bits)
            }
            TYPE_ID_DATACLASS => dataclass_attr_lookup_raw(_py, obj_ptr, attr_bits),
            _ => crate::builtins::attributes::attr_lookup_ptr_default(_py, obj_ptr, attr_bits),
        }
    }
}

pub(crate) unsafe fn attr_lookup_ptr_allow_missing(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    unsafe {
        crate::gil_assert();
        let res = if object_type_id(obj_ptr) == TYPE_ID_MODULE {
            module_attr_lookup_allow_missing(_py, obj_ptr, attr_bits)
        } else {
            attr_lookup_ptr_any(_py, obj_ptr, attr_bits)
        };
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
                dec_ref_bits(_py, result);
            }
            let _ = clear_attribute_error_if_pending(_py);
            return None;
        }
        res
    }
}

/// Function-descriptor receiver policy shared by attribute materialization and
/// immediate invocation. Only class-object slot wrappers suppress binding.
unsafe fn function_descriptor_receiver(
    function_ptr: *mut u8,
    instance_bits: Option<u64>,
) -> Option<u64> {
    unsafe {
        let bits = instance_bits?;
        let is_class = obj_from_bits(bits)
            .as_ptr()
            .is_some_and(|ptr| object_type_id(ptr) == TYPE_ID_TYPE);
        if is_class {
            let fn_ptr = crate::function_fn_ptr(function_ptr);
            if fn_ptr == fn_addr!(crate::molt_object_getattribute)
                || fn_ptr == fn_addr!(crate::molt_object_setattr)
                || fn_ptr == fn_addr!(crate::molt_object_delattr)
            {
                return None;
            }
        }
        Some(bits)
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
                if let Some(inst_bits) = function_descriptor_receiver(val_ptr, instance_bits) {
                    Some(molt_bound_method_new(val_bits, inst_bits))
                } else {
                    inc_ref_bits(_py, val_bits);
                    Some(val_bits)
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
            type_id @ (TYPE_ID_CLASSMETHOD | TYPE_ID_STATICMETHOD | TYPE_ID_PROPERTY)
                if crate::builtins::types::wrappers::wrapper_uses_default_method(
                    _py,
                    val_ptr,
                    b"__get__",
                    match type_id {
                        TYPE_ID_CLASSMETHOD => fn_addr!(crate::molt_classmethod_get),
                        TYPE_ID_STATICMETHOD => fn_addr!(crate::molt_staticmethod_get),
                        _ => fn_addr!(crate::molt_property_get),
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
        if let Some(owner) = owner {
            dec_ref_bits(_py, owner);
        }
        if let Some(instance) = instance_bits {
            dec_ref_bits(_py, instance);
        }
        dec_ref_bits(_py, val_bits);
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

/// Binding a descriptor-valued hook can recurse before a Python function is
/// entered. Charge that recursion to the existing runtime budget as well.
struct DescriptorInvocationGuard;

impl DescriptorInvocationGuard {
    fn enter(_py: &PyToken<'_>) -> Option<Self> {
        if crate::state::recursion::recursion_guard_enter() {
            Some(Self)
        } else {
            raise_exception(
                _py,
                "RecursionError",
                "maximum recursion depth exceeded while binding a descriptor",
            )
        }
    }
}

impl Drop for DescriptorInvocationGuard {
    fn drop(&mut self) {
        crate::state::recursion::recursion_guard_exit();
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
            let result = match function_descriptor_receiver(val_ptr, instance_bits) {
                Some(receiver) => {
                    // Match the receiver lifetime a materialized bound method
                    // would own, even if the callback drops its original owner.
                    inc_ref_bits(_py, receiver);
                    let result = args.call_with_receiver(_py, val_bits, receiver);
                    dec_ref_bits(_py, receiver);
                    result
                }
                None => args.call(_py, val_bits),
            };
            dec_ref_bits(_py, val_bits);
            return Some(result);
        }
        let _guard = DescriptorInvocationGuard::enter(_py)?;
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
        let Some(ptr) = maybe_ptr_from_bits(descriptor) else {
            return DescriptorMutationOutcome::NotDescriptor;
        };
        inc_ref_bits(_py, descriptor);
        inc_ref_bits(_py, instance);
        let outcome = if object_type_id(ptr) == TYPE_ID_NATIVE_DESCRIPTOR {
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
                    DescriptorMutation::Set(_) => fn_addr!(crate::molt_property_set),
                    DescriptorMutation::Delete => fn_addr!(crate::molt_property_delete),
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
        &runtime_state(_py).special_cache.awaitable_await,
        fn_addr!(molt_awaitable_await),
        1,
    )
}

pub(crate) struct SlotsInfo {
    pub(crate) allows_dict: bool,
    pub(crate) allows_weakref: bool,
}

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
        if obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(dict_name_bits)) {
            return Some(class_slots_info(_py, class_ptr).is_none_or(|info| info.allows_dict));
        }
        let weakref_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.weakref_name,
            b"__weakref__",
        );
        if obj_eq(
            _py,
            obj_from_bits(attr_bits),
            obj_from_bits(weakref_name_bits),
        ) {
            return Some(class_slots_info(_py, class_ptr).is_none_or(|info| info.allows_weakref));
        }
        None
    }
}

pub(crate) unsafe fn class_slots_info(_py: &PyToken<'_>, class_ptr: *mut u8) -> Option<SlotsInfo> {
    use crate::object::layout::{ClassSlotDeclaration, class_slot_declaration};
    unsafe {
        crate::gil_assert();
        if MoltObject::from_ptr(class_ptr).bits() == builtin_classes(_py).reference_type {
            return Some(SlotsInfo {
                allows_dict: false,
                allows_weakref: false,
            });
        }
        // An ordinary class without its own declaration acquires managed
        // dictionary/weakref storage. Public namespace changes cannot revoke it.
        if !matches!(
            class_slot_declaration(class_ptr),
            ClassSlotDeclaration::Names(_)
        ) {
            return None;
        }
        let dict_name =
            intern_static_name(_py, &runtime_state(_py).interned.dict_name, b"__dict__");
        let weakref_name = intern_static_name(
            _py,
            &runtime_state(_py).interned.weakref_name,
            b"__weakref__",
        );
        let mut info = SlotsInfo {
            allows_dict: false,
            allows_weakref: false,
        };
        for class in class_mro_view(_py, class_ptr).iter().copied() {
            let Some(ptr) = obj_from_bits(class).as_ptr() else {
                continue;
            };
            if object_type_id(ptr) != TYPE_ID_TYPE {
                continue;
            }
            match class_slot_declaration(ptr) {
                ClassSlotDeclaration::Names(names) => {
                    info.allows_dict |= slot_declaration_contains(names, dict_name);
                    info.allows_weakref |= slot_declaration_contains(names, weakref_name);
                }
                ClassSlotDeclaration::Absent | ClassSlotDeclaration::Uninitialized => {
                    // Builtin roots without managed instance layout contribute
                    // neither; ordinary unslotted bases contribute both.
                    if !is_builtin_class_bits(_py, class) {
                        info.allows_dict = true;
                        info.allows_weakref = true;
                    }
                }
            }
        }
        Some(info)
    }
}

/// Prepare owned slot provenance independently of class allocation. Dynamic
/// type construction rejects malformed declarations before a finalizing
/// metaclass can observe a new class; static sealing consumes the same authority.
pub(crate) unsafe fn prepare_class_slot_declaration(
    _py: &PyToken<'_>,
    dict_ptr: *mut u8,
) -> Option<u64> {
    unsafe {
        let name = intern_static_name(_py, &runtime_state(_py).interned.slots_name, b"__slots__");
        if exception_pending(_py) {
            return None;
        }
        let Some(slots) = dict_get_in_place(_py, dict_ptr, name) else {
            if exception_pending(_py) {
                return None;
            }
            return Some(MoltObject::none().bits());
        };
        if exception_pending(_py) {
            return None;
        }
        // Iterating __slots__ may execute Python. Retain the exact namespace
        // value before reentry can rebind/delete the borrowed dict entry.
        inc_ref_bits(_py, slots);
        let _slots_owner = obj_from_bits(slots).as_ptr().map(crate::PtrDropGuard::new);
        let names = if obj_from_bits(slots)
            .as_ptr()
            .is_some_and(|ptr| object_type_id(ptr) == TYPE_ID_STRING)
        {
            inc_ref_bits(_py, slots);
            vec![slots]
        } else {
            let Some(names) = crate::object::iterable::collect(
                _py,
                slots,
                crate::object::iterable::LengthHint::Skip,
            ) else {
                return None;
            };
            names
        };
        let valid = names.iter().copied().all(|name| {
            obj_from_bits(name)
                .as_ptr()
                .is_some_and(|ptr| object_type_id(ptr) == TYPE_ID_STRING)
        });
        if !valid {
            for name in names {
                dec_ref_bits(_py, name);
            }
            raise_exception::<()>(_py, "TypeError", "__slots__ items must be str");
            return None;
        }
        let tuple = crate::object::builders::alloc_tuple_owned(_py, &names);
        if tuple.is_null() {
            for name in names {
                dec_ref_bits(_py, name);
            }
            return None;
        }
        Some(MoltObject::from_ptr(tuple).bits())
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
        let Some(declaration) = prepare_class_slot_declaration(_py, dict_ptr) else {
            return false;
        };
        class_set_slot_declaration_owned(class_ptr, declaration);
        true
    }
}

pub(crate) unsafe fn apply_class_slots_layout(_py: &PyToken<'_>, class_ptr: *mut u8) -> bool {
    unsafe {
        crate::gil_assert();
        if class_ptr.is_null() {
            return true;
        }
        if object_type_id(class_ptr) != TYPE_ID_TYPE {
            return true;
        }
        let dict_bits = class_dict_bits(class_ptr);
        let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
            return true;
        };
        if object_type_id(dict_ptr) != TYPE_ID_DICT {
            return true;
        }
        if !capture_class_slot_declaration(_py, class_ptr, dict_ptr) {
            return false;
        }
        let crate::object::layout::ClassSlotDeclaration::Names(names) =
            crate::object::layout::class_slot_declaration(class_ptr)
        else {
            return true;
        };
        let slot_names = crate::object::seq_access::pin_tuple(
            _py,
            obj_from_bits(names)
                .as_ptr()
                .expect("captured slot declaration"),
        )
        .expect("captured slot tuple");

        let offsets_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.field_offsets_name,
            b"__molt_field_offsets__",
        );
        let layout_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.molt_layout_size,
            b"__molt_layout_size__",
        );
        let dict_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.dict_name, b"__dict__");
        let weakref_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.weakref_name,
            b"__weakref__",
        );

        let mut offsets_bits = dict_get_in_place(_py, dict_ptr, offsets_name_bits).unwrap_or(0);
        let mut offsets_owned = false;
        if obj_from_bits(offsets_bits).is_none() || offsets_bits == 0 {
            let new_ptr = alloc_dict_with_pairs(_py, &[]);
            if new_ptr.is_null() {
                return false;
            }
            offsets_bits = MoltObject::from_ptr(new_ptr).bits();
            offsets_owned = true;
            dict_set_in_place(_py, dict_ptr, offsets_name_bits, offsets_bits);
            if exception_pending(_py) {
                dec_ref_bits(_py, offsets_bits);
                return false;
            }
        }
        let Some(offsets_ptr) = obj_from_bits(offsets_bits).as_ptr() else {
            raise_exception::<()>(_py, "TypeError", "__molt_field_offsets__ must be dict");
            return false;
        };
        if object_type_id(offsets_ptr) != TYPE_ID_DICT {
            raise_exception::<()>(_py, "TypeError", "__molt_field_offsets__ must be dict");
            return false;
        }
        // Slot assignment below can perform dict equality against hostile
        // construction keys. Keep the selected map alive even if such a
        // callback rebinds or deletes the public namespace attribute.
        if !offsets_owned {
            inc_ref_bits(_py, offsets_bits);
        }
        let _offsets_owner = crate::PtrDropGuard::new(offsets_ptr);

        let mut layout_size = 0usize;
        let mut original_layout_size = 0usize;
        if let Some(size_bits) = dict_get_in_place(_py, dict_ptr, layout_name_bits)
            && let Some(size) = obj_from_bits(size_bits).as_int()
            && size > 0
        {
            layout_size = size as usize;
            original_layout_size = layout_size;
        }
        if layout_size == 0 {
            let mro = class_mro_view(_py, class_ptr);
            for base_bits in mro.iter().copied().skip(1) {
                let Some(base) = obj_from_bits(base_bits).as_ptr() else {
                    continue;
                };
                if object_type_id(base) != TYPE_ID_TYPE {
                    continue;
                }
                let inherited = if crate::object::class_definition_is_finished(base) {
                    Some(
                        crate::object::layout::class_cached_layout_size(base)
                            .expect("sealed base has no private layout size"),
                    )
                } else {
                    let Some(base_dict) = obj_from_bits(class_dict_bits(base)).as_ptr() else {
                        continue;
                    };
                    if object_type_id(base_dict) != TYPE_ID_DICT {
                        continue;
                    }
                    dict_get_in_place(_py, base_dict, layout_name_bits)
                        .and_then(|bits| obj_from_bits(bits).as_int())
                        .filter(|&size| size > 0)
                        .and_then(|size| usize::try_from(size).ok())
                };
                if let Some(inherited) = inherited {
                    layout_size = layout_size.max(inherited);
                }
            }
            original_layout_size = layout_size;
        }
        if layout_size == 0 {
            layout_size = 8;
        }

        let reserved_prefix = crate::object::class_reserved_layout_prefix(class_ptr);
        let reserved_tail = crate::object::class_reserved_layout_tail(_py, class_ptr);
        layout_size = layout_size.max(reserved_prefix.saturating_add(reserved_tail));
        layout_size = layout_size.saturating_sub(reserved_tail);

        let mut updated = false;
        let mut occupied_offsets: Vec<usize> = Vec::new();
        let mro = class_mro_view(_py, class_ptr);
        for base_bits in mro.iter().copied().skip(1) {
            let Some(base_ptr) = obj_from_bits(base_bits).as_ptr() else {
                continue;
            };
            if object_type_id(base_ptr) != TYPE_ID_TYPE {
                continue;
            }
            let Some(base_offsets_bits) =
                class_field_offsets_map_bits(_py, base_ptr, Some(offsets_name_bits))
            else {
                continue;
            };
            let Some(base_offsets_ptr) = obj_from_bits(base_offsets_bits).as_ptr() else {
                continue;
            };
            if object_type_id(base_offsets_ptr) != TYPE_ID_DICT {
                continue;
            }
            let entries = dict_order(base_offsets_ptr).clone();
            for pair in entries.chunks(2) {
                if pair.len() != 2 {
                    continue;
                }
                let key_bits = pair[0];
                let val_bits = pair[1];
                if dict_get_in_place(_py, offsets_ptr, key_bits).is_some() {
                    continue;
                }
                dict_set_in_place(_py, offsets_ptr, key_bits, val_bits);
                if let Some(offset) = obj_from_bits(val_bits).as_int() {
                    let offset = offset.max(0) as usize;
                    let end = offset.saturating_add(std::mem::size_of::<u64>());
                    if end > layout_size {
                        layout_size = end;
                    }
                    occupied_offsets.push(offset);
                }
                updated = true;
            }
        }

        let entries = dict_order(offsets_ptr).clone();
        for pair in entries.chunks(2) {
            if pair.len() != 2 {
                continue;
            }
            if let Some(offset) = obj_from_bits(pair[1]).as_int()
                && offset >= 0
            {
                let offset = offset as usize;
                occupied_offsets.push(offset);
                let end = offset.saturating_add(std::mem::size_of::<u64>());
                if end > layout_size {
                    layout_size = end;
                }
            }
        }

        for slot_bits in slot_names.iter().copied() {
            let slot_obj = obj_from_bits(slot_bits);
            if obj_eq(_py, slot_obj, obj_from_bits(dict_name_bits))
                || obj_eq(_py, slot_obj, obj_from_bits(weakref_name_bits))
            {
                continue;
            }
            let mut existing_offset = dict_get_in_place(_py, offsets_ptr, slot_bits)
                .and_then(|bits| obj_from_bits(bits).as_int())
                .and_then(|offset| {
                    if offset >= 0 {
                        Some(offset as usize)
                    } else {
                        None
                    }
                });
            if let Some(offset) = existing_offset
                && occupied_offsets
                    .iter()
                    .filter(|&&seen| seen == offset)
                    .count()
                    > 1
            {
                existing_offset = None;
            }
            let offset = if let Some(offset) = existing_offset {
                offset
            } else {
                while occupied_offsets.contains(&layout_size) {
                    layout_size = layout_size.saturating_add(std::mem::size_of::<u64>());
                }
                let offset = layout_size;
                let offset_bits = MoltObject::from_int(offset as i64).bits();
                dict_set_in_place(_py, offsets_ptr, slot_bits, offset_bits);
                updated = true;
                offset
            };
            occupied_offsets.push(offset);
            let end = offset.saturating_add(std::mem::size_of::<u64>());
            if end > layout_size {
                layout_size = end;
            }
        }
        layout_size = layout_size.saturating_add(reserved_tail);
        if layout_size != original_layout_size {
            updated = true;
        }
        if updated {
            let size_bits = MoltObject::from_int(layout_size as i64).bits();
            dict_set_in_place(_py, dict_ptr, layout_name_bits, size_bits);
        }
        if let Some(filter) = debug_class_layout_filter() {
            let class_name = string_obj_to_owned(obj_from_bits(class_name_bits(class_ptr)))
                .unwrap_or_else(|| "<unknown>".to_string());
            if debug_class_layout_match(&class_name) {
                let mut offsets_dump: Vec<String> = Vec::new();
                let entries = dict_order(offsets_ptr).clone();
                for pair in entries.chunks(2) {
                    if pair.len() != 2 {
                        continue;
                    }
                    let key_bits = pair[0];
                    let val_bits = pair[1];
                    let key = string_obj_to_owned(obj_from_bits(key_bits))
                        .unwrap_or_else(|| "<non-str>".to_string());
                    let val = obj_from_bits(val_bits).as_int().unwrap_or(-1);
                    offsets_dump.push(format!("{key}={val}"));
                }
                offsets_dump.sort();
                eprintln!(
                    "molt debug class_layout: {class_name} layout_size={} slots_filter={} offsets=[{}]",
                    layout_size,
                    filter,
                    offsets_dump.join(", ")
                );
            }
        }
        true
    }
}

pub(crate) unsafe fn object_attr_lookup_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    unsafe {
        crate::gil_assert();
        let obj_bits = MoltObject::from_ptr(obj_ptr).bits();
        let class_bits = object_class_bits(obj_ptr);
        let mut class_ptr_opt: Option<*mut u8> = None;
        let mut field_offset_resolved: Option<usize> = None;
        if class_bits == 0 {
            let await_name_bits =
                intern_static_name(_py, &runtime_state(_py).interned.await_name, b"__await__");
            if obj_eq(
                _py,
                obj_from_bits(attr_bits),
                obj_from_bits(await_name_bits),
            ) && crate::object::object_poll_fn(obj_ptr) != 0
            {
                let self_bits = MoltObject::from_ptr(obj_ptr).bits();
                let func_bits = awaitable_await_func_bits(_py);
                return Some(molt_bound_method_new(func_bits, self_bits));
            }
        }
        if class_bits != 0
            && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
            && object_type_id(class_ptr) == TYPE_ID_TYPE
        {
            class_ptr_opt = Some(class_ptr);
            if let Some(offset) = class_own_slot_field_offset(_py, class_ptr, attr_bits) {
                let bits = object_field_get_ptr_raw(_py, obj_ptr, offset);
                if is_missing_bits(_py, bits) {
                    dec_ref_bits(_py, bits);
                    return None;
                }
                return Some(bits);
            }
            let class_version = class_layout_version_bits(class_ptr);
            let mut descriptor_cache_hit = false;
            if let Some(entry) = descriptor_cache_lookup(_py, class_bits, attr_bits, class_version)
            {
                descriptor_cache_hit = true;
                if let Some(bits) = entry.data_desc_bits {
                    let bound = descriptor_bind(
                        _py,
                        bits,
                        Some(MoltObject::from_ptr(class_ptr).bits()),
                        Some(obj_bits),
                    );
                    let pending = exception_pending(_py);
                    entry.release(_py);
                    if let Some(bound) = bound {
                        return Some(bound);
                    }
                    if pending {
                        return None;
                    }
                } else {
                    entry.release(_py);
                }
            }
            if !descriptor_cache_hit {
                if let Some(val_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits) {
                    if descriptor_is_data(_py, val_bits) {
                        descriptor_cache_store(
                            _py,
                            class_bits,
                            attr_bits,
                            class_version,
                            Some(val_bits),
                            None,
                        );
                        if let Some(bound) = descriptor_bind(
                            _py,
                            val_bits,
                            Some(MoltObject::from_ptr(class_ptr).bits()),
                            Some(obj_bits),
                        ) {
                            return Some(bound);
                        }
                        if exception_pending(_py) {
                            return None;
                        }
                    } else {
                        descriptor_cache_store(
                            _py,
                            class_bits,
                            attr_bits,
                            class_version,
                            None,
                            Some(val_bits),
                        );
                    }
                } else {
                    descriptor_cache_store(_py, class_bits, attr_bits, class_version, None, None);
                }
            }
            // --- Field-offset IC fast path (CPython 3.12 LOAD_ATTR_INSTANCE_VALUE) ---
            // Try the TLS IC first to skip the expensive MRO walk in
            // `class_field_offset`.  The IC is keyed by (class_bits, attr_name
            // hash) and validated against the global type version.
            let attr_name_slice: Option<&[u8]> = obj_from_bits(attr_bits)
                .as_ptr()
                .filter(|&p| object_type_id(p) == TYPE_ID_STRING)
                .map(|p| std::slice::from_raw_parts(string_bytes(p), string_len(p)));

            let current_type_version = crate::object::global_type_version();

            if let Some(name_bytes) = attr_name_slice
                && let Some(offset) =
                    field_offset_ic_lookup(class_bits, name_bytes, current_type_version)
            {
                profile_hit_unchecked(&FIELD_OFFSET_IC_HIT_COUNT);
                field_offset_resolved = Some(offset);
            }

            if field_offset_resolved.is_none()
                && let Some(offset) = class_field_offset(_py, class_ptr, attr_bits)
            {
                profile_hit_unchecked(&FIELD_OFFSET_IC_MISS_COUNT);
                field_offset_resolved = Some(offset);
                // Populate IC for next time.
                if let Some(name_bytes) = attr_name_slice {
                    field_offset_ic_insert(class_bits, name_bytes, current_type_version, offset);
                }
            }
        }
        let class_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.class_name, b"__class__");
        if obj_eq(
            _py,
            obj_from_bits(attr_bits),
            obj_from_bits(class_name_bits),
        ) {
            if class_bits != 0 {
                inc_ref_bits(_py, class_bits);
                return Some(class_bits);
            }
            let fallback = type_of_bits(_py, MoltObject::from_ptr(obj_ptr).bits());
            inc_ref_bits(_py, fallback);
            return Some(fallback);
        }
        let dict_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.dict_name, b"__dict__");
        let weakref_name_bits = intern_static_name(
            _py,
            &runtime_state(_py).interned.weakref_name,
            b"__weakref__",
        );
        if obj_eq(
            _py,
            obj_from_bits(attr_bits),
            obj_from_bits(weakref_name_bits),
        ) {
            if let Some(class_ptr) = class_ptr_opt
                && class_instance_layout_attr_allowed(_py, class_ptr, attr_bits) == Some(false)
            {
                return None;
            }
            return Some(crate::object::weakref::weakref_head_for_target(
                _py,
                MoltObject::from_ptr(obj_ptr).bits(),
            ));
        }
        if obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(dict_name_bits)) {
            if let Some(class_ptr) = class_ptr_opt
                && class_instance_layout_attr_allowed(_py, class_ptr, attr_bits) == Some(false)
            {
                return None;
            }
            let dict_bits = crate::object::field_storage::materialize(_py, obj_ptr)?;
            inc_ref_bits(_py, dict_bits);
            return Some(dict_bits);
        }
        if let Some(val) = crate::object::accessors::instance_attribute_lookup(
            _py,
            obj_ptr,
            attr_bits,
            field_offset_resolved,
        ) {
            return Some(val);
        }
        if exception_pending(_py) {
            return None;
        }
        let class_ptr_opt = std::hint::black_box(class_ptr_opt);
        if let Some(class_ptr) = class_ptr_opt {
            let class_version = class_layout_version_bits(class_ptr);
            if let Some(entry) = descriptor_cache_lookup(_py, class_bits, attr_bits, class_version)
            {
                if entry.data_desc_bits.is_none()
                    && let Some(val_bits) = entry.class_attr_bits
                {
                    let bound = descriptor_bind(
                        _py,
                        val_bits,
                        Some(MoltObject::from_ptr(class_ptr).bits()),
                        Some(obj_bits),
                    );
                    let pending = exception_pending(_py);
                    entry.release(_py);
                    if let Some(bound) = bound {
                        return Some(bound);
                    }
                    if pending {
                        return None;
                    }
                } else {
                    entry.release(_py);
                }
            } else if let Some(val_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits) {
                if descriptor_is_data(_py, val_bits) {
                    descriptor_cache_store(
                        _py,
                        class_bits,
                        attr_bits,
                        class_version,
                        Some(val_bits),
                        None,
                    );
                    return None;
                }
                descriptor_cache_store(
                    _py,
                    class_bits,
                    attr_bits,
                    class_version,
                    None,
                    Some(val_bits),
                );
                if let Some(bound) = descriptor_bind(
                    _py,
                    val_bits,
                    Some(MoltObject::from_ptr(class_ptr).bits()),
                    Some(obj_bits),
                ) {
                    return Some(bound);
                }
                if exception_pending(_py) {
                    return None;
                }
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
/// per-instance shadow check).  Returns the resolved plain-function method plus
/// the `(class_bits, class_version)` IC key and a `can_shadow` flag.  Returns
/// `None` for any shape the unbound fast path does not cover (non-class type,
/// custom `__getattribute__`, data descriptor, non-function attr).
///
/// # Safety
/// `obj_ptr` must be live; the GIL must be held.
pub(crate) unsafe fn object_method_ic_resolve(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<MethodIcResolution> {
    unsafe {
        crate::gil_assert();
        let type_id = object_type_id(obj_ptr);
        if !crate::object::heap_kind_has_class_shape(type_id) && type_id != TYPE_ID_DATACLASS {
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
            match crate::builtins::methods::object_method_bits(_py, "__getattribute__") {
                Some(default_bits) => {
                    if !obj_eq(_py, obj_from_bits(raw_bits), obj_from_bits(default_bits)) {
                        return None;
                    }
                }
                None => return None,
            }
        }

        // (3) Resolve the class attribute, preferring the descriptor cache
        // (populated by object_attr_lookup_raw) and validating against the
        // class layout version.  A data descriptor of this name takes
        // precedence over both the instance and a plain method, so bail.
        let class_version = class_layout_version_bits(class_ptr);
        let class_attr_bits = {
            let mut resolved: Option<u64> = None;
            if let Some(entry) = descriptor_cache_lookup(_py, class_bits, attr_bits, class_version)
            {
                if entry.data_desc_bits.is_some() {
                    entry.release(_py);
                    return None;
                }
                resolved = entry.class_attr_bits;
                entry.release(_py);
            }
            match resolved {
                Some(bits) => bits,
                None => {
                    let val_bits = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)?;
                    if descriptor_is_data(_py, val_bits) {
                        descriptor_cache_store(
                            _py,
                            class_bits,
                            attr_bits,
                            class_version,
                            Some(val_bits),
                            None,
                        );
                        return None;
                    }
                    descriptor_cache_store(
                        _py,
                        class_bits,
                        attr_bits,
                        class_version,
                        None,
                        Some(val_bits),
                    );
                    val_bits
                }
            }
        };

        // (3 cont.) Only a plain function qualifies for the unbound fast path.
        let func_ptr = maybe_ptr_from_bits(class_attr_bits)?;
        if object_type_id(func_ptr) != TYPE_ID_FUNCTION {
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
        let has_field_offset = class_field_offset(_py, class_ptr, attr_bits).is_some();
        let allows_instance_dict = match class_slots_info(_py, class_ptr) {
            // A slots class permits an instance dict when it declares
            // `__dict__` or inherits a dict-bearing user class.
            Some(info) => info.allows_dict,
            // No __slots__ anywhere in the MRO => instances carry a __dict__.
            None => true,
        };
        let can_shadow = has_field_offset || allows_instance_dict;

        Some(MethodIcResolution {
            class_bits,
            class_version,
            func_bits: class_attr_bits,
            can_shadow,
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
        let offset = class_field_offset(_py, class_ptr, attr_bits);
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

/// `super().method(args)` fast path: resolve the MRO-next plain method without
/// allocating a `super` object, a bound method, or a CallArgs builder.
///
/// `start_class_bits` is the defining class (`__class__`); `self_bits` is the
/// instance.  Mirrors the `TYPE_ID_SUPER` branch of `attr_lookup_ptr`: walk the
/// MRO of `type(self)` (the object-bound super form) starting AFTER
/// `start_class`, and return the first plain-`TYPE_ID_FUNCTION` attribute found
/// in a class dict, along with the `(type(self), version)` IC key.  Returns
/// `None` (caller falls back to the allocating `super_new` + `get_attr` + `call`
/// path) for any non-function descriptor, builtin-class hit, or unsupported
/// shape.  The returned `func_bits` is BORROWED (it lives in a class dict).
///
/// # Safety
/// `self_bits` must be a live object; the GIL must be held.
pub(crate) unsafe fn super_resolve_method_unbound(
    _py: &PyToken<'_>,
    start_class_bits: u64,
    self_bits: u64,
    attr_bits: u64,
) -> Option<SuperIcResolution> {
    unsafe {
        crate::gil_assert();
        // Object-bound super: walk the MRO of `type(self)`.  (The class-bound
        // `super(C, D)` form, where the target is itself a type, is left to the
        // slow path — it is rare and outside the per-call hot loop.)
        let self_ptr = maybe_ptr_from_bits(self_bits)?;
        if object_type_id(self_ptr) == TYPE_ID_TYPE {
            return None;
        }
        let obj_type_bits = type_of_bits(_py, self_bits);
        let obj_type_ptr = obj_from_bits(obj_type_bits).as_ptr()?;
        if object_type_id(obj_type_ptr) != TYPE_ID_TYPE {
            return None;
        }
        let self_class_version = class_layout_version_bits(obj_type_ptr);
        let mro = class_mro_view(_py, obj_type_ptr);
        let mut found_start = false;
        for class_bits in mro.iter().copied() {
            if !found_start {
                if class_bits == start_class_bits {
                    found_start = true;
                }
                continue;
            }
            let Some(class_ptr) = obj_from_bits(class_bits).as_ptr() else {
                continue;
            };
            if object_type_id(class_ptr) != TYPE_ID_TYPE {
                continue;
            }
            // Builtin classes resolve methods through a separate table; defer
            // those to the slow path for exact parity.
            if is_builtin_class_bits(_py, class_bits) {
                return None;
            }
            let dict_bits = class_dict_bits(class_ptr);
            let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
                continue;
            };
            if object_type_id(dict_ptr) != TYPE_ID_DICT {
                continue;
            }
            let Some(val_bits) = dict_get_in_place(_py, dict_ptr, attr_bits) else {
                continue;
            };
            // Only a plain function qualifies; anything else (classmethod /
            // staticmethod / property / data descriptor) needs descriptor_bind.
            let val_ptr = maybe_ptr_from_bits(val_bits)?;
            if object_type_id(val_ptr) != TYPE_ID_FUNCTION {
                return None;
            }
            return Some(SuperIcResolution {
                self_class_bits: obj_type_bits,
                self_class_version,
                func_bits: val_bits,
            });
        }
        None
    }
}

pub(crate) unsafe fn dataclass_attr_lookup_raw(
    _py: &PyToken<'_>,
    obj_ptr: *mut u8,
    attr_bits: u64,
) -> Option<u64> {
    unsafe {
        crate::gil_assert();
        let obj_bits = MoltObject::from_ptr(obj_ptr).bits();
        let desc_ptr = dataclass_desc_ptr(obj_ptr);
        if desc_ptr.is_null() {
            return None;
        }
        let allows_dict = (*desc_ptr).allows_dict;
        let attr_name = string_obj_to_owned(obj_from_bits(attr_bits));
        let class_bits = object_class_bits(obj_ptr);
        let offset = attr_name
            .as_deref()
            .and_then(|name| (*desc_ptr).field_name_to_index.get(name).copied())
            .map(|index| index * std::mem::size_of::<u64>());
        if let Some(offset) = offset
            && crate::object::field_storage::field_at_offset(_py, obj_ptr, offset)
                .is_some_and(|field| field.declared_slot)
        {
            return crate::object::accessors::instance_attribute_lookup(
                _py,
                obj_ptr,
                attr_bits,
                Some(offset),
            );
        }
        if class_bits != 0
            && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
            && object_type_id(class_ptr) == TYPE_ID_TYPE
            && let Some(val_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
            && descriptor_is_data(_py, val_bits)
        {
            if let Some(bound) = descriptor_bind(
                _py,
                val_bits,
                Some(MoltObject::from_ptr(class_ptr).bits()),
                Some(obj_bits),
            ) {
                return Some(bound);
            }
            if exception_pending(_py) {
                return None;
            }
        }
        let class_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.class_name, b"__class__");
        if obj_eq(
            _py,
            obj_from_bits(attr_bits),
            obj_from_bits(class_name_bits),
        ) {
            if class_bits != 0 {
                inc_ref_bits(_py, class_bits);
                return Some(class_bits);
            }
            return None;
        }
        let dict_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.dict_name, b"__dict__");
        if obj_eq(_py, obj_from_bits(attr_bits), obj_from_bits(dict_name_bits)) {
            if allows_dict {
                let bits = crate::object::field_storage::materialize(_py, obj_ptr)?;
                inc_ref_bits(_py, bits);
                return Some(bits);
            }
            return None;
        }
        if let Some(value) =
            crate::object::accessors::instance_attribute_lookup(_py, obj_ptr, attr_bits, offset)
        {
            return Some(value);
        }
        if exception_pending(_py) {
            return None;
        }
        if class_bits != 0
            && let Some(class_ptr) = obj_from_bits(class_bits).as_ptr()
            && object_type_id(class_ptr) == TYPE_ID_TYPE
            && let Some(val_bits) = class_attr_lookup_raw_mro(_py, class_ptr, attr_bits)
        {
            if let Some(bound) = descriptor_bind(
                _py,
                val_bits,
                Some(MoltObject::from_ptr(class_ptr).bits()),
                Some(obj_bits),
            ) {
                return Some(bound);
            }
            if exception_pending(_py) {
                return None;
            }
        }
        None
    }
}
