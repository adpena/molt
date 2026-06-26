use super::*;

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_find_in_path(fullname_bits: u64, search_paths_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        importlib_find_in_path_payload(_py, fullname_bits, search_paths_bits, false)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_find_in_path_package_context(
    fullname_bits: u64,
    search_paths_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        importlib_find_in_path_payload(_py, fullname_bits, search_paths_bits, true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_find_spec(
    fullname_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
    meta_path_bits: u64,
    path_hooks_bits: u64,
    path_importer_cache_bits: u64,
    package_context_bits: u64,
    machinery_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let fullname = match string_arg_from_bits(_py, fullname_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let search_paths =
            match string_sequence_arg_from_bits(_py, search_paths_bits, "search paths") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let package_context = is_truthy(_py, obj_from_bits(package_context_bits));
        match importlib_find_spec_with_runtime_state_bits(
            _py,
            ImportlibRuntimeSpecContext {
                fullname: &fullname,
                search_paths: &search_paths,
                module_file,
                meta_path_bits,
                path_hooks_bits,
                path_importer_cache_bits,
                package_context,
                machinery_bits,
            },
        ) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_find_spec_orchestrate(
    module_name_bits: u64,
    path_snapshot_bits: u64,
    module_file_bits: u64,
    spec_cache_bits: u64,
    machinery_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let path_snapshot =
            match string_sequence_arg_from_bits(_py, path_snapshot_bits, "search paths") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(spec_cache_ptr) = obj_from_bits(spec_cache_bits).as_ptr() else {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "invalid importlib find_spec cache mapping",
            );
        };
        if unsafe { object_type_id(spec_cache_ptr) } != TYPE_ID_DICT {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "invalid importlib find_spec cache mapping",
            );
        }
        match importlib_find_spec_orchestrated_impl(
            _py,
            &module_name,
            &path_snapshot,
            module_file,
            spec_cache_ptr,
            machinery_bits,
        ) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_find_spec_from_path_hooks(
    fullname_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
    path_hooks_bits: u64,
    path_importer_cache_bits: u64,
    package_context_bits: u64,
    machinery_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let fullname = match string_arg_from_bits(_py, fullname_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let search_paths =
            match string_sequence_arg_from_bits(_py, search_paths_bits, "search paths") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let package_context = is_truthy(_py, obj_from_bits(package_context_bits));
        importlib_find_spec_from_path_hooks_impl(
            _py,
            ImportlibPathHooksContext {
                fullname: &fullname,
                search_paths: &search_paths,
                module_file,
                path_hooks_bits,
                path_importer_cache_bits,
                package_context,
                machinery_bits,
            },
        )
    })
}

pub(super) struct ImportlibPathHooksContext<'a> {
    fullname: &'a str,
    search_paths: &'a [String],
    module_file: Option<String>,
    path_hooks_bits: u64,
    path_importer_cache_bits: u64,
    package_context: bool,
    machinery_bits: u64,
}

pub(super) fn importlib_find_spec_from_path_hooks_impl(
    _py: &PyToken<'_>,
    ctx: ImportlibPathHooksContext<'_>,
) -> u64 {
    let path_hooks_count =
        match iterable_count_arg_from_bits(_py, ctx.path_hooks_bits, "path_hooks") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
    let via_path_hooks = match importlib_find_spec_via_path_hooks(
        _py,
        ctx.fullname,
        ctx.search_paths,
        ctx.path_hooks_bits,
        ctx.path_importer_cache_bits,
    ) {
        Ok(value) => value,
        Err(bits) => return bits,
    };
    if let Some(spec_bits) = via_path_hooks {
        return spec_bits;
    }
    let fs_allowed = has_capability(_py, "fs.read");
    audit_capability_decision(
        "importlib.find_spec.from_path_hooks_impl",
        "fs.read",
        AuditArgs::None,
        fs_allowed,
    );
    if ctx.fullname != "math" && !fs_allowed {
        return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
    }

    // This intrinsic models the PathFinder lane (invoked from meta-path),
    // so meta-path participation is logically non-empty even without a
    // Python sentinel object.
    let meta_path_count = 1;
    let payload = match importlib_find_spec_payload(
        _py,
        ctx.fullname,
        ctx.search_paths,
        ctx.module_file,
        meta_path_count,
        path_hooks_count,
        ctx.package_context,
    ) {
        Ok(Some(payload)) => payload,
        Ok(None) => return MoltObject::none().bits(),
        Err(bits) => return bits,
    };
    match importlib_find_spec_object_bits(_py, ctx.fullname, &payload, ctx.machinery_bits) {
        Ok(bits) => bits,
        Err(err) => err,
    }
}

pub(super) struct ImportlibRuntimeSpecContext<'a> {
    fullname: &'a str,
    search_paths: &'a [String],
    module_file: Option<String>,
    meta_path_bits: u64,
    path_hooks_bits: u64,
    path_importer_cache_bits: u64,
    package_context: bool,
    machinery_bits: u64,
}

pub(super) fn importlib_find_spec_with_runtime_state_bits(
    _py: &PyToken<'_>,
    ctx: ImportlibRuntimeSpecContext<'_>,
) -> Result<u64, u64> {
    let meta_path_count = iterable_count_arg_from_bits(_py, ctx.meta_path_bits, "meta_path")?;
    let path_hooks_count = iterable_count_arg_from_bits(_py, ctx.path_hooks_bits, "path_hooks")?;
    let via_meta_path =
        importlib_find_spec_via_meta_path(_py, ctx.fullname, ctx.search_paths, ctx.meta_path_bits)?;
    if let Some(spec_bits) = via_meta_path {
        return Ok(spec_bits);
    }
    // CPython only consults path hooks via meta-path finders (notably PathFinder).
    // If meta_path is empty, find_spec should not probe path_hooks directly.
    let via_path_hooks = if meta_path_count == 0 {
        None
    } else {
        importlib_find_spec_via_path_hooks(
            _py,
            ctx.fullname,
            ctx.search_paths,
            ctx.path_hooks_bits,
            ctx.path_importer_cache_bits,
        )?
    };
    if let Some(spec_bits) = via_path_hooks {
        return Ok(spec_bits);
    }
    let fs_allowed = has_capability(_py, "fs.read");
    audit_capability_decision(
        "importlib.find_spec.with_runtime_state_bits",
        "fs.read",
        AuditArgs::None,
        fs_allowed,
    );
    if ctx.fullname != "math" && !fs_allowed {
        return Err(raise_exception::<_>(
            _py,
            "PermissionError",
            "missing fs.read capability",
        ));
    }
    let payload = match importlib_find_spec_payload(
        _py,
        ctx.fullname,
        ctx.search_paths,
        ctx.module_file,
        meta_path_count,
        path_hooks_count,
        ctx.package_context,
    )? {
        Some(payload) => payload,
        None => return Ok(MoltObject::none().bits()),
    };
    importlib_find_spec_object_bits(_py, ctx.fullname, &payload, ctx.machinery_bits)
}

pub(super) fn importlib_find_spec_orchestrated_search_paths(
    _py: &PyToken<'_>,
    module_name: &str,
    modules_bits: u64,
    path_snapshot: &[String],
    module_file: Option<String>,
    spec_cache_ptr: *mut u8,
    machinery_bits: u64,
) -> Result<(Vec<String>, bool), u64> {
    let parent_payload = importlib_parent_search_paths_payload(_py, module_name, modules_bits)?;
    if !parent_payload.has_parent {
        return Ok((importlib_search_paths(path_snapshot, module_file), false));
    }
    if !parent_payload.needs_parent_spec {
        return Ok((parent_payload.search_paths, parent_payload.package_context));
    }

    let Some(parent_name) = parent_payload.parent_name else {
        return Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            "invalid importlib parent search paths payload: parent_name",
        ));
    };

    let parent_spec_bits = importlib_find_spec_orchestrated_impl(
        _py,
        &parent_name,
        path_snapshot,
        module_file.clone(),
        spec_cache_ptr,
        machinery_bits,
    )?;
    if obj_from_bits(parent_spec_bits).is_none() {
        return Ok((Vec::new(), true));
    }
    let submodule_search_locations_name = intern_static_name(
        _py,
        runtime_static_name_slot(_py, b"submodule_search_locations"),
        b"submodule_search_locations",
    );
    let parent_paths_bits =
        match getattr_optional_bits(_py, parent_spec_bits, submodule_search_locations_name) {
            Ok(Some(bits)) => bits,
            Ok(None) => MoltObject::none().bits(),
            Err(err) => {
                if !obj_from_bits(parent_spec_bits).is_none() {
                    dec_ref_bits(_py, parent_spec_bits);
                }
                return Err(err);
            }
        };
    if !obj_from_bits(parent_spec_bits).is_none() {
        dec_ref_bits(_py, parent_spec_bits);
    }
    let search_paths = match importlib_coerce_search_paths_values(
        _py,
        parent_paths_bits,
        "invalid parent package search path",
    ) {
        Ok(value) => value,
        Err(err) => {
            if !obj_from_bits(parent_paths_bits).is_none() {
                dec_ref_bits(_py, parent_paths_bits);
            }
            return Err(err);
        }
    };
    if !obj_from_bits(parent_paths_bits).is_none() {
        dec_ref_bits(_py, parent_paths_bits);
    }
    Ok((search_paths, true))
}

pub(super) fn importlib_find_spec_orchestrated_impl(
    _py: &PyToken<'_>,
    module_name: &str,
    path_snapshot: &[String],
    module_file: Option<String>,
    spec_cache_ptr: *mut u8,
    machinery_bits: u64,
) -> Result<u64, u64> {
    let runtime_state = importlib_runtime_state_view_bits(_py)?;
    let out = (|| -> Result<u64, u64> {
        let existing_spec_bits = importlib_existing_spec_from_modules_bits(
            _py,
            module_name,
            runtime_state.modules_bits,
            machinery_bits,
        )?;
        if !obj_from_bits(existing_spec_bits).is_none() {
            return Ok(existing_spec_bits);
        }

        let (search_paths, package_context) = importlib_find_spec_orchestrated_search_paths(
            _py,
            module_name,
            runtime_state.modules_bits,
            path_snapshot,
            module_file.clone(),
            spec_cache_ptr,
            machinery_bits,
        )?;
        let search_paths_bits = importlib_alloc_string_tuple_bits(_py, &search_paths)?;
        let meta_path_sig_bits = match importlib_finder_signature_tuple_bits(
            _py,
            runtime_state.meta_path_bits,
            "invalid meta_path iterable",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, search_paths_bits);
                return Err(err);
            }
        };
        let path_hooks_sig_bits = match importlib_finder_signature_tuple_bits(
            _py,
            runtime_state.path_hooks_bits,
            "invalid path_hooks iterable",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, search_paths_bits);
                dec_ref_bits(_py, meta_path_sig_bits);
                return Err(err);
            }
        };
        let path_importer_cache_sig_bits = match importlib_path_importer_cache_signature_tuple_bits(
            _py,
            runtime_state.path_importer_cache_bits,
            "invalid path_importer_cache mapping",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, search_paths_bits);
                dec_ref_bits(_py, meta_path_sig_bits);
                dec_ref_bits(_py, path_hooks_sig_bits);
                return Err(err);
            }
        };
        let module_name_bits = match alloc_str_bits(_py, module_name) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, search_paths_bits);
                dec_ref_bits(_py, meta_path_sig_bits);
                dec_ref_bits(_py, path_hooks_sig_bits);
                dec_ref_bits(_py, path_importer_cache_sig_bits);
                return Err(err);
            }
        };
        let cache_key_ptr = alloc_tuple(
            _py,
            &[
                module_name_bits,
                search_paths_bits,
                meta_path_sig_bits,
                path_hooks_sig_bits,
                path_importer_cache_sig_bits,
                MoltObject::from_bool(package_context).bits(),
            ],
        );
        if !obj_from_bits(module_name_bits).is_none() {
            dec_ref_bits(_py, module_name_bits);
        }
        if !obj_from_bits(search_paths_bits).is_none() {
            dec_ref_bits(_py, search_paths_bits);
        }
        if !obj_from_bits(meta_path_sig_bits).is_none() {
            dec_ref_bits(_py, meta_path_sig_bits);
        }
        if !obj_from_bits(path_hooks_sig_bits).is_none() {
            dec_ref_bits(_py, path_hooks_sig_bits);
        }
        if !obj_from_bits(path_importer_cache_sig_bits).is_none() {
            dec_ref_bits(_py, path_importer_cache_sig_bits);
        }
        if cache_key_ptr.is_null() {
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        let cache_key_bits = MoltObject::from_ptr(cache_key_ptr).bits();
        let cached_bits = unsafe { dict_get_in_place(_py, spec_cache_ptr, cache_key_bits) };
        if exception_pending(_py) {
            dec_ref_bits(_py, cache_key_bits);
            return Err(MoltObject::none().bits());
        }
        if let Some(cached_bits) = cached_bits {
            if !obj_from_bits(cached_bits).is_none() {
                inc_ref_bits(_py, cached_bits);
            }
            dec_ref_bits(_py, cache_key_bits);
            return Ok(cached_bits);
        }

        let spec_bits = importlib_find_spec_with_runtime_state_bits(
            _py,
            ImportlibRuntimeSpecContext {
                fullname: module_name,
                search_paths: &search_paths,
                module_file,
                meta_path_bits: runtime_state.meta_path_bits,
                path_hooks_bits: runtime_state.path_hooks_bits,
                path_importer_cache_bits: runtime_state.path_importer_cache_bits,
                package_context,
                machinery_bits,
            },
        )?;
        unsafe {
            dict_set_in_place(_py, spec_cache_ptr, cache_key_bits, spec_bits);
        }
        if exception_pending(_py) {
            if !obj_from_bits(spec_bits).is_none() {
                dec_ref_bits(_py, spec_bits);
            }
            dec_ref_bits(_py, cache_key_bits);
            return Err(MoltObject::none().bits());
        }
        dec_ref_bits(_py, cache_key_bits);
        Ok(spec_bits)
    })();
    for bits in [
        runtime_state.modules_bits,
        runtime_state.meta_path_bits,
        runtime_state.path_hooks_bits,
        runtime_state.path_importer_cache_bits,
    ] {
        if !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
    }
    out
}

pub(super) fn importlib_sys_module_bits(_py: &PyToken<'_>) -> Option<u64> {
    let module_cache = crate::builtins::exceptions::internals::module_cache(_py);
    let guard = module_cache.lock().unwrap();
    guard.get("sys").copied()
}

pub(super) fn importlib_machinery_module_file(
    _py: &PyToken<'_>,
    machinery_bits: u64,
) -> Result<Option<String>, u64> {
    let file_name = intern_runtime_static_name(_py, b"__file__");
    let file_bits = getattr_optional_bits(_py, machinery_bits, file_name)?;
    match file_bits {
        Some(bits) => {
            let out = match module_file_from_bits(_py, bits) {
                Ok(value) => value,
                Err(err) => {
                    if !obj_from_bits(bits).is_none() {
                        dec_ref_bits(_py, bits);
                    }
                    return Err(err);
                }
            };
            if !obj_from_bits(bits).is_none() {
                dec_ref_bits(_py, bits);
            }
            Ok(out)
        }
        None => Ok(None),
    }
}

pub(super) fn importlib_runtime_path_hooks_and_cache_bits(
    _py: &PyToken<'_>,
    sys_bits: Option<u64>,
) -> Result<(u64, bool, u64, bool), u64> {
    let mut path_hooks_bits = MoltObject::none().bits();
    let mut owns_path_hooks = false;
    let mut path_importer_cache_bits = MoltObject::none().bits();
    let mut owns_path_importer_cache = false;

    if let Some(sys_bits) = sys_bits
        && !obj_from_bits(sys_bits).is_none()
    {
        let path_hooks_name = intern_runtime_static_name(_py, b"path_hooks");
        let hooks_attr = getattr_optional_bits(_py, sys_bits, path_hooks_name)?;
        if let Some(bits) = hooks_attr {
            path_hooks_bits = bits;
            owns_path_hooks = true;
        } else {
            let empty_hooks_ptr = alloc_tuple(_py, &[]);
            if empty_hooks_ptr.is_null() {
                return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
            }
            path_hooks_bits = MoltObject::from_ptr(empty_hooks_ptr).bits();
            owns_path_hooks = true;
        }

        let path_importer_cache_name = intern_runtime_static_name(_py, b"path_importer_cache");
        let cache_attr = getattr_optional_bits(_py, sys_bits, path_importer_cache_name)?;
        if let Some(bits) = cache_attr {
            path_importer_cache_bits = bits;
            owns_path_importer_cache = true;
        }
    }

    if !owns_path_hooks {
        let empty_hooks_ptr = alloc_tuple(_py, &[]);
        if empty_hooks_ptr.is_null() {
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        path_hooks_bits = MoltObject::from_ptr(empty_hooks_ptr).bits();
        owns_path_hooks = true;
    }

    Ok((
        path_hooks_bits,
        owns_path_hooks,
        path_importer_cache_bits,
        owns_path_importer_cache,
    ))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_pathfinder_find_spec(
    fullname_bits: u64,
    path_bits: u64,
    machinery_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let fullname = match string_arg_from_bits(_py, fullname_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let sys_bits = importlib_sys_module_bits(_py);
        let module_file = match importlib_machinery_module_file(_py, machinery_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };

        let package_context = !obj_from_bits(path_bits).is_none();
        let search_paths: Vec<String> = if package_context {
            match importlib_coerce_search_paths_values(
                _py,
                path_bits,
                "invalid parent package search path",
            ) {
                Ok(value) => value,
                Err(bits) => return bits,
            }
        } else if let Some(sys_bits) = sys_bits {
            if obj_from_bits(sys_bits).is_none() {
                Vec::new()
            } else {
                let path_name = intern_runtime_static_name(_py, b"path");
                let path_attr = match getattr_optional_bits(_py, sys_bits, path_name) {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
                match path_attr {
                    Some(bits) => {
                        let out = match string_sequence_arg_from_bits(_py, bits, "search paths") {
                            Ok(value) => value,
                            Err(err) => {
                                if !obj_from_bits(bits).is_none() {
                                    dec_ref_bits(_py, bits);
                                }
                                return err;
                            }
                        };
                        if !obj_from_bits(bits).is_none() {
                            dec_ref_bits(_py, bits);
                        }
                        out
                    }
                    None => Vec::new(),
                }
            }
        } else {
            Vec::new()
        };

        let (path_hooks_bits, owns_path_hooks, path_importer_cache_bits, owns_path_importer_cache) =
            match importlib_runtime_path_hooks_and_cache_bits(_py, sys_bits) {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let result = importlib_find_spec_from_path_hooks_impl(
            _py,
            ImportlibPathHooksContext {
                fullname: &fullname,
                search_paths: &search_paths,
                module_file,
                path_hooks_bits,
                path_importer_cache_bits,
                package_context,
                machinery_bits,
            },
        );
        if owns_path_hooks && !obj_from_bits(path_hooks_bits).is_none() {
            dec_ref_bits(_py, path_hooks_bits);
        }
        if owns_path_importer_cache && !obj_from_bits(path_importer_cache_bits).is_none() {
            dec_ref_bits(_py, path_importer_cache_bits);
        }
        result
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_filefinder_find_spec(
    fullname_bits: u64,
    path_bits: u64,
    machinery_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let fullname = match string_arg_from_bits(_py, fullname_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let sys_bits = importlib_sys_module_bits(_py);
        let module_file = match importlib_machinery_module_file(_py, machinery_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let (path_hooks_bits, owns_path_hooks, path_importer_cache_bits, owns_path_importer_cache) =
            match importlib_runtime_path_hooks_and_cache_bits(_py, sys_bits) {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let search_paths = vec![path];
        let result = importlib_find_spec_from_path_hooks_impl(
            _py,
            ImportlibPathHooksContext {
                fullname: &fullname,
                search_paths: &search_paths,
                module_file,
                path_hooks_bits,
                path_importer_cache_bits,
                package_context: true,
                machinery_bits,
            },
        );
        if owns_path_hooks && !obj_from_bits(path_hooks_bits).is_none() {
            dec_ref_bits(_py, path_hooks_bits);
        }
        if owns_path_importer_cache && !obj_from_bits(path_importer_cache_bits).is_none() {
            dec_ref_bits(_py, path_importer_cache_bits);
        }
        result
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_invalidate_caches() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Some(util_bits) = importlib_module_cache_lookup_bits(_py, "importlib.util")
            && !obj_from_bits(util_bits).is_none()
        {
            importlib_clear_mapping_attr_best_effort(
                _py,
                util_bits,
                runtime_static_name_slot(_py, b"_SPEC_CACHE"),
                b"_SPEC_CACHE",
            );
        }
        if let Some(sys_bits) = importlib_module_cache_lookup_bits(_py, "sys")
            && !obj_from_bits(sys_bits).is_none()
        {
            importlib_clear_mapping_attr_best_effort(
                _py,
                sys_bits,
                runtime_static_name_slot(_py, b"path_importer_cache"),
                b"path_importer_cache",
            );
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_filefinder_invalidate(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(sys_bits) = importlib_module_cache_lookup_bits(_py, "sys") else {
            return MoltObject::none().bits();
        };
        if obj_from_bits(sys_bits).is_none() {
            return MoltObject::none().bits();
        }
        let path_importer_cache_name = intern_runtime_static_name(_py, b"path_importer_cache");
        let path_importer_cache_bits =
            match getattr_optional_bits(_py, sys_bits, path_importer_cache_name) {
                Ok(Some(bits)) => bits,
                Ok(None) => return MoltObject::none().bits(),
                Err(_) => {
                    if exception_pending(_py) {
                        clear_exception(_py);
                    }
                    return MoltObject::none().bits();
                }
            };

        if let Some(path_importer_cache_ptr) = obj_from_bits(path_importer_cache_bits).as_ptr() {
            if unsafe { object_type_id(path_importer_cache_ptr) } == TYPE_ID_DICT {
                let path_key_bits = match alloc_str_bits(_py, &path) {
                    Ok(bits) => bits,
                    Err(err) => {
                        if !obj_from_bits(path_importer_cache_bits).is_none() {
                            dec_ref_bits(_py, path_importer_cache_bits);
                        }
                        return err;
                    }
                };
                importlib_dict_del_string_key(_py, path_importer_cache_ptr, path_key_bits);
                if !obj_from_bits(path_key_bits).is_none() {
                    dec_ref_bits(_py, path_key_bits);
                }
            } else {
                let pop_result = match importlib_lookup_callable_attr(
                    _py,
                    path_importer_cache_bits,
                    runtime_static_name_slot(_py, b"pop"),
                    b"pop",
                ) {
                    Ok(Some(pop_bits)) => {
                        let path_key_bits = match alloc_str_bits(_py, &path) {
                            Ok(bits) => bits,
                            Err(err) => {
                                if !obj_from_bits(pop_bits).is_none() {
                                    dec_ref_bits(_py, pop_bits);
                                }
                                if !obj_from_bits(path_importer_cache_bits).is_none() {
                                    dec_ref_bits(_py, path_importer_cache_bits);
                                }
                                return err;
                            }
                        };
                        let out = call_callable_positional(
                            _py,
                            pop_bits,
                            &[path_key_bits, MoltObject::none().bits()],
                        );
                        if !obj_from_bits(pop_bits).is_none() {
                            dec_ref_bits(_py, pop_bits);
                        }
                        if !obj_from_bits(path_key_bits).is_none() {
                            dec_ref_bits(_py, path_key_bits);
                        }
                        out
                    }
                    Ok(None) => Ok(MoltObject::none().bits()),
                    Err(_) => {
                        if exception_pending(_py) {
                            clear_exception(_py);
                        }
                        Ok(MoltObject::none().bits())
                    }
                };
                if let Ok(result_bits) = pop_result
                    && !obj_from_bits(result_bits).is_none()
                {
                    dec_ref_bits(_py, result_bits);
                }
                if exception_pending(_py) {
                    clear_exception(_py);
                }
            }
        }
        if !obj_from_bits(path_importer_cache_bits).is_none() {
            dec_ref_bits(_py, path_importer_cache_bits);
        }
        MoltObject::none().bits()
    })
}
