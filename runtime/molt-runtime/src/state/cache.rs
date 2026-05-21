use crate::PyToken;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use super::RuntimeState;
use crate::object::{HEADER_FLAG_INTERNED, header_from_obj_ptr, obj_from_bits};
use crate::{MoltObject, alloc_string, init_atomic_bits};

pub(crate) struct InternedNames {
    pub(crate) bases_name: AtomicU64,
    pub(crate) mro_name: AtomicU64,
    pub(crate) get_name: AtomicU64,
    pub(crate) set_name: AtomicU64,
    pub(crate) delete_name: AtomicU64,
    pub(crate) set_name_method: AtomicU64,
    pub(crate) getattr_name: AtomicU64,
    pub(crate) getattribute_name: AtomicU64,
    pub(crate) call_name: AtomicU64,
    pub(crate) await_name: AtomicU64,
    pub(crate) iter_name: AtomicU64,
    pub(crate) next_name: AtomicU64,
    pub(crate) init_name: AtomicU64,
    pub(crate) init_subclass_name: AtomicU64,
    pub(crate) new_name: AtomicU64,
    pub(crate) instancecheck_name: AtomicU64,
    pub(crate) subclasscheck_name: AtomicU64,
    pub(crate) enter_name: AtomicU64,
    pub(crate) exit_name: AtomicU64,
    pub(crate) setattr_name: AtomicU64,
    pub(crate) delattr_name: AtomicU64,
    pub(crate) write_name: AtomicU64,
    pub(crate) flush_name: AtomicU64,
    pub(crate) readline_name: AtomicU64,
    pub(crate) sys_name: AtomicU64,
    pub(crate) sys_version_info: AtomicU64,
    pub(crate) sys_version: AtomicU64,
    pub(crate) stdout_name: AtomicU64,
    pub(crate) stdin_name: AtomicU64,
    pub(crate) modules_name: AtomicU64,
    pub(crate) all_name: AtomicU64,
    pub(crate) fspath_name: AtomicU64,
    pub(crate) dict_name: AtomicU64,
    pub(crate) slots_name: AtomicU64,
    pub(crate) weakref_name: AtomicU64,
    pub(crate) molt_dict_data_name: AtomicU64,
    pub(crate) class_name: AtomicU64,
    pub(crate) annotations_name: AtomicU64,
    pub(crate) annotate_name: AtomicU64,
    pub(crate) field_offsets_name: AtomicU64,
    pub(crate) molt_layout_size: AtomicU64,
    pub(crate) float_name: AtomicU64,
    pub(crate) index_name: AtomicU64,
    pub(crate) int_name: AtomicU64,
    pub(crate) round_name: AtomicU64,
    pub(crate) floor_name: AtomicU64,
    pub(crate) ceil_name: AtomicU64,
    pub(crate) trunc_name: AtomicU64,
    pub(crate) repr_name: AtomicU64,
    pub(crate) str_name: AtomicU64,
    pub(crate) format_name: AtomicU64,
    pub(crate) qualname_name: AtomicU64,
    pub(crate) name_name: AtomicU64,
    pub(crate) wrapped_name: AtomicU64,
    pub(crate) obj_name: AtomicU64,
    pub(crate) f_lasti_name: AtomicU64,
    pub(crate) f_code_name: AtomicU64,
    pub(crate) f_lineno_name: AtomicU64,
    pub(crate) f_locals_name: AtomicU64,
    pub(crate) tb_frame_name: AtomicU64,
    pub(crate) tb_lineno_name: AtomicU64,
    pub(crate) tb_next_name: AtomicU64,
    pub(crate) notes_name: AtomicU64,
    pub(crate) molt_arg_names: AtomicU64,
    pub(crate) molt_posonly: AtomicU64,
    pub(crate) molt_kwonly_names: AtomicU64,
    pub(crate) molt_vararg: AtomicU64,
    pub(crate) molt_varkw: AtomicU64,
    pub(crate) molt_closure_size: AtomicU64,
    pub(crate) molt_is_coroutine: AtomicU64,
    pub(crate) molt_is_generator: AtomicU64,
    pub(crate) molt_is_async_generator: AtomicU64,
    pub(crate) molt_bind_kind: AtomicU64,
    pub(crate) defaults_name: AtomicU64,
    pub(crate) kwdefaults_name: AtomicU64,
    pub(crate) abstractmethods_name: AtomicU64,
    pub(crate) lt_name: AtomicU64,
    pub(crate) le_name: AtomicU64,
    pub(crate) gt_name: AtomicU64,
    pub(crate) ge_name: AtomicU64,
    pub(crate) eq_name: AtomicU64,
    pub(crate) ne_name: AtomicU64,
    pub(crate) hash_name: AtomicU64,
    pub(crate) add_name: AtomicU64,
    pub(crate) radd_name: AtomicU64,
    pub(crate) mul_name: AtomicU64,
    pub(crate) rmul_name: AtomicU64,
    pub(crate) sub_name: AtomicU64,
    pub(crate) rsub_name: AtomicU64,
    pub(crate) truediv_name: AtomicU64,
    pub(crate) rtruediv_name: AtomicU64,
    pub(crate) floordiv_name: AtomicU64,
    pub(crate) rfloordiv_name: AtomicU64,
    pub(crate) mod_name: AtomicU64,
    pub(crate) rmod_name: AtomicU64,
    pub(crate) or_name: AtomicU64,
    pub(crate) ror_name: AtomicU64,
    pub(crate) and_name: AtomicU64,
    pub(crate) rand_name: AtomicU64,
    pub(crate) xor_name: AtomicU64,
    pub(crate) rxor_name: AtomicU64,
    pub(crate) iadd_name: AtomicU64,
    pub(crate) isub_name: AtomicU64,
    pub(crate) itruediv_name: AtomicU64,
    pub(crate) ifloordiv_name: AtomicU64,
    pub(crate) imod_name: AtomicU64,
    pub(crate) pow_name: AtomicU64,
    pub(crate) rpow_name: AtomicU64,
    pub(crate) ipow_name: AtomicU64,
    pub(crate) ilshift_name: AtomicU64,
    pub(crate) irshift_name: AtomicU64,
    pub(crate) matmul_name: AtomicU64,
    pub(crate) rmatmul_name: AtomicU64,
    pub(crate) imatmul_name: AtomicU64,
    pub(crate) ior_name: AtomicU64,
    pub(crate) iand_name: AtomicU64,
    pub(crate) ixor_name: AtomicU64,
    pub(crate) bytes_dunder: AtomicU64,
}

impl InternedNames {
    pub(crate) fn new() -> Self {
        Self {
            bases_name: AtomicU64::new(0),
            mro_name: AtomicU64::new(0),
            get_name: AtomicU64::new(0),
            set_name: AtomicU64::new(0),
            delete_name: AtomicU64::new(0),
            set_name_method: AtomicU64::new(0),
            getattr_name: AtomicU64::new(0),
            getattribute_name: AtomicU64::new(0),
            call_name: AtomicU64::new(0),
            await_name: AtomicU64::new(0),
            iter_name: AtomicU64::new(0),
            next_name: AtomicU64::new(0),
            init_name: AtomicU64::new(0),
            init_subclass_name: AtomicU64::new(0),
            new_name: AtomicU64::new(0),
            instancecheck_name: AtomicU64::new(0),
            subclasscheck_name: AtomicU64::new(0),
            enter_name: AtomicU64::new(0),
            exit_name: AtomicU64::new(0),
            setattr_name: AtomicU64::new(0),
            delattr_name: AtomicU64::new(0),
            write_name: AtomicU64::new(0),
            flush_name: AtomicU64::new(0),
            readline_name: AtomicU64::new(0),
            sys_name: AtomicU64::new(0),
            sys_version_info: AtomicU64::new(0),
            sys_version: AtomicU64::new(0),
            stdout_name: AtomicU64::new(0),
            stdin_name: AtomicU64::new(0),
            modules_name: AtomicU64::new(0),
            all_name: AtomicU64::new(0),
            fspath_name: AtomicU64::new(0),
            dict_name: AtomicU64::new(0),
            slots_name: AtomicU64::new(0),
            weakref_name: AtomicU64::new(0),
            molt_dict_data_name: AtomicU64::new(0),
            class_name: AtomicU64::new(0),
            annotations_name: AtomicU64::new(0),
            annotate_name: AtomicU64::new(0),
            field_offsets_name: AtomicU64::new(0),
            molt_layout_size: AtomicU64::new(0),
            float_name: AtomicU64::new(0),
            index_name: AtomicU64::new(0),
            int_name: AtomicU64::new(0),
            round_name: AtomicU64::new(0),
            floor_name: AtomicU64::new(0),
            ceil_name: AtomicU64::new(0),
            trunc_name: AtomicU64::new(0),
            repr_name: AtomicU64::new(0),
            str_name: AtomicU64::new(0),
            format_name: AtomicU64::new(0),
            qualname_name: AtomicU64::new(0),
            name_name: AtomicU64::new(0),
            wrapped_name: AtomicU64::new(0),
            obj_name: AtomicU64::new(0),
            f_lasti_name: AtomicU64::new(0),
            f_code_name: AtomicU64::new(0),
            f_lineno_name: AtomicU64::new(0),
            f_locals_name: AtomicU64::new(0),
            tb_frame_name: AtomicU64::new(0),
            tb_lineno_name: AtomicU64::new(0),
            tb_next_name: AtomicU64::new(0),
            notes_name: AtomicU64::new(0),
            molt_arg_names: AtomicU64::new(0),
            molt_posonly: AtomicU64::new(0),
            molt_kwonly_names: AtomicU64::new(0),
            molt_vararg: AtomicU64::new(0),
            molt_varkw: AtomicU64::new(0),
            molt_closure_size: AtomicU64::new(0),
            molt_is_coroutine: AtomicU64::new(0),
            molt_is_generator: AtomicU64::new(0),
            molt_is_async_generator: AtomicU64::new(0),
            molt_bind_kind: AtomicU64::new(0),
            defaults_name: AtomicU64::new(0),
            kwdefaults_name: AtomicU64::new(0),
            abstractmethods_name: AtomicU64::new(0),
            lt_name: AtomicU64::new(0),
            le_name: AtomicU64::new(0),
            gt_name: AtomicU64::new(0),
            ge_name: AtomicU64::new(0),
            eq_name: AtomicU64::new(0),
            ne_name: AtomicU64::new(0),
            hash_name: AtomicU64::new(0),
            add_name: AtomicU64::new(0),
            radd_name: AtomicU64::new(0),
            mul_name: AtomicU64::new(0),
            rmul_name: AtomicU64::new(0),
            sub_name: AtomicU64::new(0),
            rsub_name: AtomicU64::new(0),
            truediv_name: AtomicU64::new(0),
            rtruediv_name: AtomicU64::new(0),
            floordiv_name: AtomicU64::new(0),
            rfloordiv_name: AtomicU64::new(0),
            mod_name: AtomicU64::new(0),
            rmod_name: AtomicU64::new(0),
            or_name: AtomicU64::new(0),
            ror_name: AtomicU64::new(0),
            and_name: AtomicU64::new(0),
            rand_name: AtomicU64::new(0),
            xor_name: AtomicU64::new(0),
            rxor_name: AtomicU64::new(0),
            iadd_name: AtomicU64::new(0),
            isub_name: AtomicU64::new(0),
            itruediv_name: AtomicU64::new(0),
            ifloordiv_name: AtomicU64::new(0),
            imod_name: AtomicU64::new(0),
            pow_name: AtomicU64::new(0),
            rpow_name: AtomicU64::new(0),
            ipow_name: AtomicU64::new(0),
            ilshift_name: AtomicU64::new(0),
            irshift_name: AtomicU64::new(0),
            matmul_name: AtomicU64::new(0),
            rmatmul_name: AtomicU64::new(0),
            imatmul_name: AtomicU64::new(0),
            ior_name: AtomicU64::new(0),
            iand_name: AtomicU64::new(0),
            ixor_name: AtomicU64::new(0),
            bytes_dunder: AtomicU64::new(0),
        }
    }
}

macro_rules! define_method_cache {
    (@unit $field:ident) => {
        ()
    };
    ($($field:ident),+ $(,)?) => {
        const METHOD_CACHE_SLOT_COUNT: usize = <[()]>::len(&[
            $(define_method_cache!(@unit $field)),+
        ]);

        pub(crate) struct MethodCache {
            $(pub(crate) $field: AtomicU64,)+
        }

        impl MethodCache {
            pub(crate) fn new() -> Self {
                Self {
                    $($field: AtomicU64::new(0),)+
                }
            }

            fn slots(&self) -> Vec<&AtomicU64> {
                let mut slots = Vec::with_capacity(METHOD_CACHE_SLOT_COUNT);
                $(slots.push(&self.$field);)+
                slots
            }
        }
    };
}

define_method_cache! {
    dict_keys,
    dict_values,
    dict_items,
    dict_get,
    dict_pop,
    dict_clear,
    dict_copy,
    dict_popitem,
    dict_setdefault,
    dict_update,
    dict_fromkeys,
    dict_getitem,
    dict_setitem,
    dict_delitem,
    dict_iter,
    dict_len,
    dict_contains,
    dict_reversed,
    set_add,
    set_discard,
    set_remove,
    set_pop,
    set_clear,
    set_update,
    set_union,
    set_intersection,
    set_difference,
    set_symdiff,
    set_intersection_update,
    set_difference_update,
    set_symdiff_update,
    set_isdisjoint,
    set_issubset,
    set_issuperset,
    set_copy,
    set_iter,
    set_len,
    set_contains,
    frozenset_union,
    frozenset_intersection,
    frozenset_difference,
    frozenset_symdiff,
    frozenset_isdisjoint,
    frozenset_issubset,
    frozenset_issuperset,
    frozenset_copy,
    frozenset_iter,
    frozenset_len,
    frozenset_contains,
    tuple_new,
    list_append,
    list_extend,
    list_insert,
    list_remove,
    list_pop,
    list_clear,
    list_init,
    list_copy,
    list_reverse,
    list_count,
    list_index,
    list_sort,
    list_add,
    list_mul,
    list_rmul,
    list_iadd,
    list_imul,
    list_getitem,
    list_setitem,
    list_delitem,
    list_iter,
    list_len,
    list_contains,
    list_reversed,
    str_iter,
    str_len,
    str_str,
    str_contains,
    str_count,
    str_startswith,
    str_endswith,
    str_find,
    str_rfind,
    str_index,
    str_rindex,
    str_capitalize,
    str_title,
    str_format,
    str_format_map,
    str_isidentifier,
    str_isdigit,
    str_isdecimal,
    str_isnumeric,
    str_isspace,
    str_isalpha,
    str_isalnum,
    str_islower,
    str_isupper,
    str_isascii,
    str_istitle,
    str_isprintable,
    str_upper,
    str_lower,
    str_casefold,
    str_swapcase,
    str_strip,
    str_lstrip,
    str_rstrip,
    str_split,
    str_rsplit,
    str_splitlines,
    str_partition,
    str_rpartition,
    str_replace,
    str_removeprefix,
    str_removesuffix,
    str_zfill,
    str_center,
    str_ljust,
    str_rjust,
    str_expandtabs,
    str_join,
    str_translate,
    str_maketrans,
    str_encode,
    bytes_iter,
    bytes_len,
    bytes_contains,
    bytes_count,
    bytes_startswith,
    bytes_endswith,
    bytes_find,
    bytes_rfind,
    bytes_split,
    bytes_rsplit,
    bytes_reversed,
    bytes_strip,
    bytes_lstrip,
    bytes_rstrip,
    bytes_splitlines,
    bytes_partition,
    bytes_rpartition,
    bytes_replace,
    bytes_join,
    bytes_upper,
    bytes_lower,
    bytes_hex,
    bytes_decode,
    bytes_translate,
    bytes_maketrans,
    bytearray_iter,
    bytearray_len,
    bytearray_contains,
    bytearray_extend,
    bytearray_clear,
    bytearray_count,
    bytearray_startswith,
    bytearray_endswith,
    bytearray_find,
    bytearray_rfind,
    bytearray_split,
    bytearray_rsplit,
    bytearray_reversed,
    bytearray_strip,
    bytearray_lstrip,
    bytearray_rstrip,
    bytearray_splitlines,
    bytearray_partition,
    bytearray_rpartition,
    bytearray_replace,
    bytearray_append,
    bytearray_hex,
    bytearray_decode,
    bytearray_translate,
    bytearray_maketrans,
    bytearray_setitem,
    bytearray_delitem,
    int_new,
    int_int,
    int_index,
    int_bit_length,
    int_to_bytes,
    int_from_bytes,
    slice_indices,
    slice_hash,
    slice_eq,
    slice_reduce,
    slice_reduce_ex,
    memoryview_tobytes,
    memoryview_tolist,
    memoryview_cast,
    memoryview_setitem,
    memoryview_delitem,
    file_read,
    file_readline,
    file_readlines,
    file_read1,
    file_readall,
    file_readinto,
    file_readinto1,
    file_write,
    file_writelines,
    file_flush,
    file_close,
    file_detach,
    file_reconfigure,
    file_seek,
    file_tell,
    file_fileno,
    file_truncate,
    file_readable,
    file_writable,
    file_seekable,
    file_isatty,
    file_iter,
    file_next,
    file_enter,
    file_exit,
    file_peek,
    file_getvalue,
    file_getbuffer,
    file_io_new,
    file_io_init,
    buffered_new,
    buffered_init,
    text_io_wrapper_new,
    text_io_wrapper_init,
    bytes_io_new,
    bytes_io_init,
    string_io_new,
    string_io_init,
    generator_iter,
    generator_next,
    generator_send,
    generator_throw,
    generator_close,
    coroutine_close,
    asyncgen_aiter,
    asyncgen_anext,
    asyncgen_asend,
    asyncgen_athrow,
    asyncgen_aclose,
    property_getter,
    property_setter,
    property_deleter,
    complex_conjugate,
    object_getattribute,
    object_new,
    object_init,
    object_init_subclass,
    object_setattr,
    object_delattr,
    object_eq,
    object_ne,
    object_repr,
    object_str,
    type_getattribute,
    type_call,
    type_new,
    type_init,
    type_prepare,
    type_mro,
    type_instancecheck,
    type_subclasscheck,
    exception_init,
    exception_new,
    exception_add_note,
    exception_group_init,
    exception_group_new,
    exception_group_subgroup,
    exception_group_split,
    exception_group_derive,
    generic_alias_new,
    generic_alias_class_getitem,
    function_descriptor_get,
}

pub(crate) fn intern_static_name(_py: &PyToken<'_>, slot: &AtomicU64, name: &'static [u8]) -> u64 {
    init_atomic_bits(_py, slot, || {
        let ptr = alloc_string(_py, name);
        if ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(ptr).bits()
        }
    })
}

pub(crate) fn clear_atomic_bits(_py: &PyToken<'_>, slot: &AtomicU64) {
    crate::gil_assert();
    let bits = slot.swap(0, AtomicOrdering::AcqRel);
    if bits != 0 {
        if let Some(ptr) = obj_from_bits(bits).as_ptr() {
            let flags = unsafe { (*header_from_obj_ptr(ptr)).flags };
            if (flags & HEADER_FLAG_INTERNED) != 0 {
                return;
            }
        }
        crate::object::release_shutdown_bits(_py, bits);
    }
}

pub(crate) fn clear_atomic_slots(_py: &PyToken<'_>, slots: &[&AtomicU64]) {
    crate::gil_assert();
    for slot in slots {
        clear_atomic_bits(_py, slot);
    }
}

pub(crate) fn clear_method_cache(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
    let slots = state.method_cache.slots();
    clear_atomic_slots(_py, &slots);
}

#[cfg(test)]
mod tests {
    use super::{METHOD_CACHE_SLOT_COUNT, MethodCache, clear_method_cache};
    use crate::{MoltObject, alloc_string, runtime_state};
    use std::collections::HashSet;
    use std::sync::atomic::{AtomicU64, Ordering};

    #[test]
    fn method_cache_slots_are_manifest_complete_and_unique() {
        let cache = MethodCache::new();
        let slots = cache.slots();
        assert_eq!(slots.len(), METHOD_CACHE_SLOT_COUNT);

        let mut seen = HashSet::with_capacity(slots.len());
        for slot in slots {
            let addr = slot as *const AtomicU64 as usize;
            assert!(seen.insert(addr));
            assert_eq!(slot.load(Ordering::Acquire), 0);
        }
    }

    #[test]
    fn clear_method_cache_clears_previously_omitted_slots() {
        let _guard = crate::TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        crate::with_gil_entry_nopanic!(_py, {
            let state = runtime_state(_py);
            clear_method_cache(_py, state);

            let sentinels: [(&AtomicU64, &'static [u8]); 10] = [
                (&state.method_cache.set_clear, b"method-cache-set-clear"),
                (&state.method_cache.set_union, b"method-cache-set-union"),
                (
                    &state.method_cache.frozenset_union,
                    b"method-cache-frozenset-union",
                ),
                (&state.method_cache.tuple_new, b"method-cache-tuple-new"),
                (&state.method_cache.list_init, b"method-cache-list-init"),
                (&state.method_cache.list_add, b"method-cache-list-add"),
                (&state.method_cache.str_str, b"method-cache-str-str"),
                (&state.method_cache.str_find, b"method-cache-str-find"),
                (&state.method_cache.str_rfind, b"method-cache-str-rfind"),
                (&state.method_cache.str_index, b"method-cache-str-index"),
            ];

            for (slot, name) in sentinels {
                let ptr = alloc_string(_py, name);
                assert!(!ptr.is_null());
                slot.store(MoltObject::from_ptr(ptr).bits(), Ordering::Release);
            }

            clear_method_cache(_py, state);

            for (slot, _) in sentinels {
                assert_eq!(slot.load(Ordering::Acquire), 0);
            }
        });
    }
}
