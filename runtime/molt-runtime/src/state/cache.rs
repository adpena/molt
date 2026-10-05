use crate::PyToken;
use std::sync::atomic::{AtomicU64, Ordering as AtomicOrdering};

use super::RuntimeState;
use crate::{MoltObject, alloc_string, dec_ref_bits, init_atomic_bits};

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
    sys_version_info,
    sys_version,
    stdout_name,
    stdin_name,
    modules_name,
    dunder_builtins_name,
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
    f_builtins_name,
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
    molt_bind_kind,
    defaults_name,
    kwdefaults_name,
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
    initializing_name => b"_initializing",
    dunder_suppress_context_name => b"__suppress_context__",
    exec_module_name => b"exec_module",
    exists_name => b"exists",
    extension_file_loader_name => b"ExtensionFileLoader",
    extension_suffixes_name => b"EXTENSION_SUFFIXES",
    file_finder_name => b"FileFinder",
    files_name => b"files",
    find_spec_name => b"find_spec",
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
    loader_name => b"loader",
    load_module_name => b"load_module",
    load_module_shim_name => b"_load_module_shim",
    magic_number_name => b"MAGIC_NUMBER",
    meta_path_finder_name => b"MetaPathFinder",
    meta_path_name => b"meta_path",
    modules_name => b"modules",
    module_from_spec_name => b"module_from_spec",
    module_spec_name => b"ModuleSpec",
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
            if !crate::exception_pending(_py) {
                crate::raise_exception::<u64>(_py, "MemoryError", "static name allocation failed");
            }
            0
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
pub(crate) fn intern_bridge_protocol_name(
    _py: &PyToken<'_>,
    key: &[u8],
) -> Result<Option<u64>, ()> {
    if crate::exception_pending(_py) {
        return Err(());
    }
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
        _ => return Ok(None),
    };
    let bits = intern_static_name(_py, slot, name);
    if bits == 0 || crate::exception_pending(_py) {
        Err(())
    } else {
        Ok(Some(bits))
    }
}

#[cfg(feature = "stdlib_logging_ext")]
pub(crate) fn intern_bridge_write_name(_py: &PyToken<'_>, key: &[u8]) -> Result<Option<u64>, ()> {
    if crate::exception_pending(_py) {
        return Err(());
    }
    if key != b"write" {
        return Ok(None);
    }
    let bits = intern_static_name(
        _py,
        &crate::runtime_state(_py).interned.write_name,
        b"write",
    );
    if bits == 0 || crate::exception_pending(_py) {
        Err(())
    } else {
        Ok(Some(bits))
    }
}

/// Allocate one owned keyword-marker object for a runtime cache publication.
/// Zero is the cache initialization failure value; Python None is never a
/// marker candidate. Both itertools bridge profiles and functools use this
/// allocator, preserving the original allocation error and leaving retry open.
pub(crate) fn alloc_kwd_mark(py: &PyToken<'_>) -> u64 {
    if crate::exception_pending(py) {
        return 0;
    }
    let ptr = crate::alloc_object(
        py,
        std::mem::size_of::<crate::MoltHeader>(),
        crate::TYPE_ID_OBJECT,
    );
    if ptr.is_null() {
        0
    } else {
        MoltObject::from_ptr(ptr).bits()
    }
}

/// Give a Python-callable result its own reference to a borrowed cached handle.
/// Cache initialization transfers an owner to the slot; neither a cache hit nor
/// its first publication transfers that owner to the caller. A zero handle is
/// failed initialization: return None without replacing the pending exception.
/// Retaining tuple/dict publication already owns borrowed inputs and does not
/// use this result boundary.
pub(crate) fn retain_cached_result(py: &PyToken<'_>, bits: u64) -> u64 {
    if bits == 0 {
        return MoltObject::none().bits();
    }
    crate::inc_ref_bits(py, bits);
    bits
}

fn release_atomic_cache_owner(_py: &PyToken<'_>, bits: u64) {
    if bits != 0 {
        dec_ref_bits(_py, bits);
    }
}

/// Return whether a populated slot was detached, including interned values
/// whose physical lifetime remains owned by the shutdown singleton authority.
#[cfg(test)]
pub(crate) fn clear_atomic_bits(_py: &PyToken<'_>, slot: &AtomicU64) -> bool {
    clear_atomic_slots(_py, &[slot])
}

pub(crate) fn clear_atomic_slots(_py: &PyToken<'_>, slots: &[&AtomicU64]) -> bool {
    crate::gil_assert();
    // Publish the entire empty cohort before a displaced owner's finalizer can
    // reenter any sibling cache. New callback publications belong to the next
    // shutdown fixed-point pass, not this detached snapshot.
    let mut detached = Vec::with_capacity(slots.len());
    for slot in slots {
        let bits = slot.swap(0, AtomicOrdering::AcqRel);
        if bits != 0 {
            detached.push(bits);
        }
    }
    let changed = !detached.is_empty();
    for bits in detached {
        release_atomic_cache_owner(_py, bits);
    }
    changed
}

/// Enumerate canonical class anchors only from declared runtime cache owners.
/// Heap immutability alone is never permission to retire an arbitrary class.
pub(crate) fn cached_runtime_class_roots(py: &PyToken<'_>, slots: &[&AtomicU64]) -> Vec<u64> {
    crate::gil_assert();
    slots
        .iter()
        .map(|slot| slot.load(AtomicOrdering::Acquire))
        .filter(|bits| crate::object::class_storage::is_canonical_runtime_class(py, *bits))
        .collect()
}

/// Keep canonical class anchors live through the shared retirement transaction;
/// detach every other cache owner before any callback-capable reference release.
pub(crate) fn clear_cached_runtime_callbacks(py: &PyToken<'_>, slots: &[&AtomicU64]) -> bool {
    crate::gil_assert();
    let callbacks: Vec<_> = slots
        .iter()
        .copied()
        .filter(|slot| {
            !crate::object::class_storage::is_canonical_runtime_class(
                py,
                slot.load(AtomicOrdering::Acquire),
            )
        })
        .collect();
    clear_atomic_slots(py, &callbacks)
}

pub(crate) fn clear_method_cache(_py: &PyToken<'_>, state: &RuntimeState) -> bool {
    crate::gil_assert();
    let slots = state.method_cache.slots();
    clear_atomic_slots(_py, &slots)
}

pub(crate) fn clear_runtime_static_names(_py: &PyToken<'_>, state: &RuntimeState) -> bool {
    crate::gil_assert();
    let slots = state.runtime_static_names.slots();
    clear_atomic_slots(_py, &slots)
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

    #[cfg(any(
        feature = "stdlib_math",
        feature = "stdlib_serial",
        feature = "stdlib_logging_ext"
    ))]
    #[test]
    fn bridge_name_failure_is_distinct_from_an_unsupported_name() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            #[cfg(any(feature = "stdlib_math", feature = "stdlib_serial"))]
            assert_eq!(
                super::intern_bridge_protocol_name(py, b"unsupported-name"),
                Ok(None)
            );
            #[cfg(feature = "stdlib_logging_ext")]
            assert_eq!(
                super::intern_bridge_write_name(py, b"unsupported-name"),
                Ok(None)
            );
            crate::raise_exception::<u64>(py, "MemoryError", "original bridge allocation failure");
            let original = crate::exception_last_bits_noinc(py);
            #[cfg(any(feature = "stdlib_math", feature = "stdlib_serial"))]
            for key in [b"__float__".as_slice(), b"unsupported-name"] {
                assert_eq!(super::intern_bridge_protocol_name(py, key), Err(()));
            }
            #[cfg(feature = "stdlib_logging_ext")]
            for key in [b"write".as_slice(), b"unsupported-name"] {
                assert_eq!(super::intern_bridge_write_name(py, key), Err(()));
            }
            assert_eq!(crate::exception_last_bits_noinc(py), original);
            let _ = crate::molt_exception_clear();
        });
    }

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
    fn clear_method_cache_releases_every_remaining_private_callback() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(_py, {
            let state = runtime_state(_py);
            clear_method_cache(_py, state);
            for slot in state.method_cache.slots() {
                let ptr = alloc_string(_py, b"private callback cache sentinel");
                assert!(!ptr.is_null());
                slot.store(MoltObject::from_ptr(ptr).bits(), Ordering::Release);
            }
            clear_method_cache(_py, state);
            for slot in state.method_cache.slots() {
                assert_eq!(slot.load(Ordering::Acquire), 0);
            }
        });
    }

    #[test]
    fn clear_runtime_static_names_releases_every_manifest_slot() {
        let _guard = crate::test_support::RuntimeTestTransaction::new();
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
