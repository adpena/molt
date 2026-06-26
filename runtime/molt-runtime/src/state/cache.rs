use crate::PyToken;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use super::RuntimeState;
use crate::object::{HEADER_FLAG_INTERNED, header_from_obj_ptr, obj_from_bits};
use crate::{MoltObject, alloc_string, init_atomic_bits};

macro_rules! define_interned_names {
    (@unit $field:ident) => {
        ()
    };
    ($($field:ident),+ $(,)?) => {
        const INTERNED_NAME_SLOT_COUNT: usize = <[()]>::len(&[
            $(define_interned_names!(@unit $field)),+
        ]);

        pub(crate) struct InternedNames {
            $(pub(crate) $field: AtomicU64,)+
        }

        impl InternedNames {
            pub(crate) fn new() -> Self {
                Self {
                    $($field: AtomicU64::new(0),)+
                }
            }

            pub(crate) fn slots(&self) -> Vec<&AtomicU64> {
                let mut slots = Vec::with_capacity(INTERNED_NAME_SLOT_COUNT);
                $(slots.push(&self.$field);)+
                slots
            }
        }
    };
}

define_interned_names! {
    bases_name,
    mro_name,
    get_name,
    set_name,
    delete_name,
    set_name_method,
    getattr_name,
    getattribute_name,
    call_name,
    await_name,
    iter_name,
    next_name,
    init_name,
    init_subclass_name,
    new_name,
    instancecheck_name,
    subclasscheck_name,
    enter_name,
    exit_name,
    setattr_name,
    delattr_name,
    handle_name,
    write_name,
    flush_name,
    readline_name,
    sys_name,
    sys_version_info,
    sys_version,
    stdout_name,
    stdin_name,
    modules_name,
    all_name,
    fspath_name,
    dict_name,
    slots_name,
    weakref_name,
    molt_dict_data_name,
    class_name,
    annotations_name,
    annotate_name,
    field_offsets_name,
    molt_layout_size,
    float_name,
    index_name,
    int_name,
    bool_name,
    round_name,
    floor_name,
    ceil_name,
    trunc_name,
    abs_name,
    len_name,
    repr_name,
    str_name,
    format_name,
    qualname_name,
    name_name,
    wrapped_name,
    obj_name,
    f_back_name,
    f_lasti_name,
    f_code_name,
    f_lineno_name,
    f_globals_name,
    f_locals_name,
    filename_name,
    lineno_name,
    plain_name,
    line_name,
    end_lineno_name,
    colno_name,
    end_colno_name,
    tb_frame_name,
    tb_lasti_name,
    tb_lineno_name,
    tb_next_name,
    notes_name,
    molt_arg_names,
    molt_posonly,
    molt_kwonly_names,
    molt_vararg,
    molt_varkw,
    molt_closure_size,
    molt_is_coroutine,
    molt_is_generator,
    molt_is_async_generator,
    molt_bind_kind,
    defaults_name,
    kwdefaults_name,
    abstractmethods_name,
    lt_name,
    le_name,
    gt_name,
    ge_name,
    eq_name,
    ne_name,
    hash_name,
    add_name,
    radd_name,
    mul_name,
    rmul_name,
    sub_name,
    rsub_name,
    truediv_name,
    rtruediv_name,
    floordiv_name,
    rfloordiv_name,
    mod_name,
    rmod_name,
    lshift_name,
    rlshift_name,
    rshift_name,
    rrshift_name,
    or_name,
    ror_name,
    and_name,
    rand_name,
    xor_name,
    rxor_name,
    iadd_name,
    isub_name,
    imul_name,
    itruediv_name,
    ifloordiv_name,
    imod_name,
    pow_name,
    rpow_name,
    ipow_name,
    ilshift_name,
    irshift_name,
    matmul_name,
    rmatmul_name,
    imatmul_name,
    ior_name,
    iand_name,
    ixor_name,
    bytes_dunder,
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
    str_hash,
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
    int_hash,
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
    object_dir,
    object_format,
    object_hash,
    object_getstate,
    object_lt,
    object_le,
    object_gt,
    object_ge,
    int_abs,
    int_add,
    int_and,
    int_bool,
    int_ceil,
    int_divmod,
    str_add,
    str_getitem,
    bytes_index,
    bytes_rindex,
    bytes_removeprefix,
    bytes_removesuffix,
    bytes_capitalize,
    bytes_swapcase,
    bytes_title,
    bytes_isalpha,
    bytes_isalnum,
    bytes_isdigit,
    bytes_isspace,
    bytes_islower,
    bytes_isupper,
    bytes_istitle,
    bytes_isascii,
    bytes_zfill,
    bytes_center,
    bytes_ljust,
    bytes_rjust,
    bytes_expandtabs,
    bytearray_insert,
    bytearray_pop,
    bytearray_remove,
    bytearray_reverse,
    bytearray_resize,
    bytearray_copy,
    bytearray_index,
    bytearray_rindex,
    bytearray_removeprefix,
    bytearray_removesuffix,
    bytearray_join,
    bytearray_capitalize,
    bytearray_upper,
    bytearray_lower,
    bytearray_swapcase,
    bytearray_title,
    bytearray_isalpha,
    bytearray_isalnum,
    bytearray_isdigit,
    bytearray_isspace,
    bytearray_islower,
    bytearray_isupper,
    bytearray_istitle,
    bytearray_isascii,
    bytearray_zfill,
    bytearray_center,
    bytearray_ljust,
    bytearray_rjust,
    bytearray_expandtabs,
    int_bit_count,
    int_as_integer_ratio,
    int_conjugate,
    int_is_integer,
    float_new,
    float_hash,
    float_float,
    float_as_integer_ratio,
    float_conjugate,
    float_hex,
    float_is_integer,
    float_fromhex,
    float_from_number,
    complex_from_number,
    memoryview_from_flags,
    memoryview_count,
    memoryview_index,
    memoryview_hex,
    memoryview_release,
    memoryview_toreadonly,
    range_count,
    range_index,
    function_descriptor_get,
}

macro_rules! define_runtime_static_names {
    (@unit $field:ident => $name:literal) => {
        ()
    };
    ($($field:ident => $name:literal),+ $(,)?) => {
        const RUNTIME_STATIC_NAME_SLOT_COUNT: usize = <[()]>::len(&[
            $(define_runtime_static_names!(@unit $field => $name)),+
        ]);

        pub(crate) struct RuntimeStaticNames {
            $(pub(crate) $field: AtomicU64,)+
        }

        impl RuntimeStaticNames {
            pub(crate) fn new() -> Self {
                Self {
                    $($field: AtomicU64::new(0),)+
                }
            }

            pub(crate) fn slot_for(&self, name: &'static [u8]) -> Option<&AtomicU64> {
                match name {
                    $($name => Some(&self.$field),)+
                    _ => None,
                }
            }

            fn slots(&self) -> Vec<&AtomicU64> {
                let mut slots = Vec::with_capacity(RUNTIME_STATIC_NAME_SLOT_COUNT);
                $(slots.push(&self.$field);)+
                slots
            }
        }
    };
}

define_runtime_static_names! {
    any_name => b"Any",
    builtin_importer_name => b"BuiltinImporter",
    bytecode_suffixes_name => b"BYTECODE_SUFFIXES",
    cached_name => b"cached",
    cache_from_source_name => b"cache_from_source",
    certfile_name => b"certfile",
    clear_name => b"clear",
    close_name => b"close",
    contents_name => b"contents",
    create_module_name => b"create_module",
    debug_bytecode_suffixes_name => b"DEBUG_BYTECODE_SUFFIXES",
    decode_name => b"decode",
    decode_source_name => b"decode_source",
    dict_name => b"Dict",
    dunder_cached_name => b"__cached__",
    dunder_file_name => b"__file__",
    dunder_loader_name => b"__loader__",
    dunder_name_name => b"__name__",
    dunder_package_name => b"__package__",
    dunder_path_name => b"__path__",
    dunder_spec_name => b"__spec__",
    dunder_suppress_context_name => b"__suppress_context__",
    exec_module_name => b"exec_module",
    exists_name => b"exists",
    extension_file_loader_name => b"ExtensionFileLoader",
    extension_suffixes_name => b"EXTENSION_SUFFIXES",
    file_finder_name => b"FileFinder",
    files_name => b"files",
    find_spec_name => b"find_spec",
    frozen_importer_name => b"FrozenImporter",
    generic_name => b"Generic",
    get_resource_reader_name => b"get_resource_reader",
    get_source_name => b"get_source",
    has_location_name => b"has_location",
    intrinsic_lookup_name => b"_molt_intrinsic_lookup",
    intrinsics_name => b"_molt_intrinsics",
    is_dir_name => b"is_dir",
    is_file_name => b"is_file",
    is_package_name => b"is_package",
    is_resource_name => b"is_resource",
    iterator_name => b"Iterator",
    iterdir_name => b"iterdir",
    joinpath_name => b"joinpath",
    keyfile_name => b"keyfile",
    list_name => b"List",
    loader_basics_name => b"_LoaderBasics",
    loader_name => b"loader",
    load_module_name => b"load_module",
    load_module_shim_name => b"_load_module_shim",
    magic_number_name => b"MAGIC_NUMBER",
    meta_path_finder_name => b"MetaPathFinder",
    meta_path_name => b"meta_path",
    modules_name => b"modules",
    module_from_spec_name => b"module_from_spec",
    module_spec_name => b"ModuleSpec",
    molt_loader_name => b"_MOLT_LOADER",
    molt_roots_name => b"molt_roots",
    name_name => b"name",
    namespace_loader_name => b"NamespaceLoader",
    open_name => b"open",
    open_resource_name => b"open_resource",
    optimized_bytecode_suffixes_name => b"OPTIMIZED_BYTECODE_SUFFIXES",
    optional_name => b"Optional",
    origin_name => b"origin",
    overload_name => b"overload",
    param_spec_args_name => b"_ParamSpecArgs",
    param_spec_kwargs_name => b"_ParamSpecKwargs",
    param_spec_name => b"_ParamSpec",
    parent_name => b"parent",
    path_finder_name => b"PathFinder",
    path_hooks_name => b"path_hooks",
    path_importer_cache_name => b"path_importer_cache",
    path_name => b"path",
    pop_name => b"pop",
    private_file_loader_name => b"_FileLoader",
    private_source_loader_name => b"_SourceLoader",
    protocol_name => b"Protocol",
    read_name => b"read",
    resource_path_name => b"resource_path",
    runtime_name => b"_molt_runtime",
    source_file_loader_name => b"SourceFileLoader",
    source_from_cache_name => b"source_from_cache",
    source_suffixes_name => b"SOURCE_SUFFIXES",
    sourceless_file_loader_name => b"SourcelessFileLoader",
    spec_cache_name => b"_SPEC_CACHE",
    spec_from_file_location_name => b"spec_from_file_location",
    spec_from_loader_name => b"spec_from_loader",
    submodule_search_locations_name => b"submodule_search_locations",
    suppress_name => b"suppress",
    type_alias_type_name => b"_MoltTypeAlias",
    type_var_name => b"_TypeVar",
    type_var_tuple_name => b"_TypeVarTuple",
    union_name => b"Union",
    windows_registry_finder_name => b"WindowsRegistryFinder",
    zip_source_loader_name => b"_ZipSourceLoader",
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

pub(crate) fn runtime_static_name_slot(
    _py: &PyToken<'_>,
    name: &'static [u8],
) -> &'static AtomicU64 {
    crate::runtime_state(_py)
        .runtime_static_names
        .slot_for(name)
        .unwrap_or_else(|| {
            panic!(
                "runtime static name slot missing for {}",
                String::from_utf8_lossy(name)
            )
        })
}

pub(crate) fn intern_runtime_static_name(_py: &PyToken<'_>, name: &'static [u8]) -> u64 {
    let slot = runtime_static_name_slot(_py, name);
    intern_static_name(_py, slot, name)
}

#[cfg(any(feature = "stdlib_math", feature = "stdlib_serial"))]
pub(crate) fn intern_bridge_protocol_name(_py: &PyToken<'_>, key: &[u8]) -> Option<u64> {
    let interned = &crate::runtime_state(_py).interned;
    let (slot, name): (&AtomicU64, &'static [u8]) = match key {
        b"__float__" => (&interned.float_name, b"__float__"),
        b"__index__" => (&interned.index_name, b"__index__"),
        b"__trunc__" => (&interned.trunc_name, b"__trunc__"),
        b"__ceil__" => (&interned.ceil_name, b"__ceil__"),
        b"__floor__" => (&interned.floor_name, b"__floor__"),
        b"__round__" => (&interned.round_name, b"__round__"),
        b"__int__" => (&interned.int_name, b"__int__"),
        b"__bool__" => (&interned.bool_name, b"__bool__"),
        b"__abs__" => (&interned.abs_name, b"__abs__"),
        b"__len__" => (&interned.len_name, b"__len__"),
        _ => return None,
    };
    Some(intern_static_name(_py, slot, name))
}

#[cfg(feature = "stdlib_logging_ext")]
pub(crate) fn intern_bridge_write_name(_py: &PyToken<'_>, key: &[u8]) -> Option<u64> {
    match key {
        b"write" => Some(intern_static_name(
            _py,
            &crate::runtime_state(_py).interned.write_name,
            b"write",
        )),
        _ => None,
    }
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

pub(crate) fn clear_runtime_static_names(_py: &PyToken<'_>, state: &RuntimeState) {
    crate::gil_assert();
    let slots = state.runtime_static_names.slots();
    clear_atomic_slots(_py, &slots);
}

#[cfg(test)]
mod tests {
    use super::{
        INTERNED_NAME_SLOT_COUNT, InternedNames, METHOD_CACHE_SLOT_COUNT, MethodCache,
        RUNTIME_STATIC_NAME_SLOT_COUNT, RuntimeStaticNames, clear_method_cache,
        clear_runtime_static_names, intern_runtime_static_name,
    };
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
    fn interned_name_slots_are_manifest_complete_and_unique() {
        let names = InternedNames::new();
        let slots = names.slots();
        assert_eq!(slots.len(), INTERNED_NAME_SLOT_COUNT);

        let mut seen = HashSet::with_capacity(slots.len());
        for slot in slots {
            let addr = slot as *const AtomicU64 as usize;
            assert!(seen.insert(addr));
            assert_eq!(slot.load(Ordering::Acquire), 0);
        }
    }

    #[test]
    fn runtime_static_name_slots_are_manifest_complete_and_unique() {
        let names = RuntimeStaticNames::new();
        let slots = names.slots();
        assert_eq!(slots.len(), RUNTIME_STATIC_NAME_SLOT_COUNT);

        let mut seen = HashSet::with_capacity(slots.len());
        for slot in slots {
            let addr = slot as *const AtomicU64 as usize;
            assert!(seen.insert(addr));
            assert_eq!(slot.load(Ordering::Acquire), 0);
        }

        assert!(names.slot_for(b"__spec__").is_some());
        assert!(names.slot_for(b"path_importer_cache").is_some());
        assert!(names.slot_for(b"certfile").is_some());
        assert!(names.slot_for(b"missing-runtime-static-name").is_none());
    }

    #[test]
    fn clear_method_cache_clears_previously_omitted_slots() {
        let _guard = crate::TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        crate::with_gil_entry_nopanic!(_py, {
            let state = runtime_state(_py);
            clear_method_cache(_py, state);

            let sentinels: [(&AtomicU64, &'static [u8]); 15] = [
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
                (&state.method_cache.object_dir, b"method-cache-object-dir"),
                (&state.method_cache.int_abs, b"method-cache-int-abs"),
                (&state.method_cache.str_add, b"method-cache-str-add"),
                (&state.method_cache.bytes_index, b"method-cache-bytes-index"),
                (
                    &state.method_cache.bytearray_upper,
                    b"method-cache-bytearray-upper",
                ),
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

    #[test]
    fn clear_runtime_static_names_releases_every_manifest_slot() {
        let _guard = crate::TEST_MUTEX.lock().unwrap_or_else(|e| e.into_inner());
        crate::with_gil_entry_nopanic!(_py, {
            let state = runtime_state(_py);
            clear_runtime_static_names(_py, state);

            let spec_bits = intern_runtime_static_name(_py, b"__spec__");
            let cache_bits = intern_runtime_static_name(_py, b"path_importer_cache");
            assert_ne!(spec_bits, 0);
            assert_ne!(cache_bits, 0);
            assert_ne!(spec_bits, cache_bits);

            clear_runtime_static_names(_py, state);

            for slot in state.runtime_static_names.slots() {
                assert_eq!(slot.load(Ordering::Acquire), 0);
            }
        });
    }
}
