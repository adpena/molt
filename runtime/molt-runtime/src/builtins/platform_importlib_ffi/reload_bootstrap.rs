use super::*;

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_reload(
    module_bits: u64,
    util_bits: u64,
    machinery_bits: u64,
    import_module_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mut name_bits = MoltObject::none().bits();
        let mut module_loader_bits = MoltObject::none().bits();
        let mut module_loader_owned = false;
        let mut modules_bits = MoltObject::none().bits();

        let out = (|| -> Result<u64, u64> {
            let module_name_name = intern_runtime_static_name(_py, b"__name__");
            let spec_name = intern_runtime_static_name(_py, b"__spec__");
            let mut module_name_bits =
                if let Some(spec_bits) = getattr_optional_bits(_py, module_bits, spec_name)? {
                    let out = getattr_optional_bits(_py, spec_bits, module_name_name)?;
                    if !obj_from_bits(spec_bits).is_none() {
                        dec_ref_bits(_py, spec_bits);
                    }
                    out
                } else {
                    None
                };
            if module_name_bits.is_none() {
                module_name_bits = getattr_optional_bits(_py, module_bits, module_name_name)?;
            }
            let Some(module_name_bits) = module_name_bits else {
                return Err(raise_exception::<_>(
                    _py,
                    "TypeError",
                    "reload() argument must be a module",
                ));
            };

            modules_bits = importlib_runtime_modules_bits(_py)?;
            let Some(modules_ptr) = obj_from_bits(modules_bits).as_ptr() else {
                if !obj_from_bits(module_name_bits).is_none() {
                    dec_ref_bits(_py, module_name_bits);
                }
                return Err(importlib_modules_runtime_error(_py));
            };
            let in_sys_modules = unsafe { dict_get_in_place(_py, modules_ptr, module_name_bits) }
                .is_some_and(|bits| bits == module_bits);
            if exception_pending(_py) {
                if !obj_from_bits(module_name_bits).is_none() {
                    dec_ref_bits(_py, module_name_bits);
                }
                return Err(MoltObject::none().bits());
            }
            if !in_sys_modules {
                let display = format_obj_str(_py, obj_from_bits(module_name_bits));
                if !obj_from_bits(module_name_bits).is_none() {
                    dec_ref_bits(_py, module_name_bits);
                }
                return Err(raise_exception::<_>(
                    _py,
                    "ImportError",
                    &format!("module {display} not in sys.modules"),
                ));
            }

            let module_name_obj = obj_from_bits(module_name_bits);
            let Some(module_name) = string_obj_to_owned(module_name_obj) else {
                let module_name_type = type_name(_py, module_name_obj);
                if !module_name_obj.is_none() {
                    dec_ref_bits(_py, module_name_bits);
                }
                return Err(raise_exception::<_>(
                    _py,
                    "AttributeError",
                    &format!("'{module_name_type}' object has no attribute 'rpartition'"),
                ));
            };
            if !module_name_obj.is_none() {
                dec_ref_bits(_py, module_name_bits);
            }
            name_bits = alloc_str_bits(_py, &module_name)?;

            let module_file_name = intern_runtime_static_name(_py, b"__file__");
            let module_file = match getattr_optional_bits(_py, module_bits, module_file_name)? {
                Some(bits) => {
                    let out = string_obj_to_owned(obj_from_bits(bits));
                    if !obj_from_bits(bits).is_none() {
                        dec_ref_bits(_py, bits);
                    }
                    out
                }
                None => None,
            };

            let loader_name = intern_runtime_static_name(_py, b"loader");
            if let Some(spec_bits) = getattr_optional_bits(_py, module_bits, spec_name)? {
                if !obj_from_bits(spec_bits).is_none()
                    && let Some(loader_bits) = getattr_optional_bits(_py, spec_bits, loader_name)?
                    && !obj_from_bits(loader_bits).is_none()
                {
                    module_loader_bits = loader_bits;
                    module_loader_owned = true;
                }
                if !obj_from_bits(spec_bits).is_none() {
                    dec_ref_bits(_py, spec_bits);
                }
            }

            if let Some(module_file) = module_file {
                let module_file_bits = alloc_str_bits(_py, &module_file)?;
                let mut locations_bits = MoltObject::none().bits();
                let mut locations_owned = false;
                let path_name = intern_runtime_static_name(_py, b"__path__");
                if let Some(path_bits) = getattr_optional_bits(_py, module_bits, path_name)? {
                    if let Some(path_ptr) = obj_from_bits(path_bits).as_ptr() {
                        let type_id = unsafe { object_type_id(path_ptr) };
                        if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                            locations_bits = importlib_list_from_iterable(
                                _py,
                                path_bits,
                                "submodule_search_locations",
                            )?;
                            locations_owned = true;
                        }
                    }
                    if !obj_from_bits(path_bits).is_none() {
                        dec_ref_bits(_py, path_bits);
                    }
                }

                let mut loader_override_bits = module_loader_bits;
                if !obj_from_bits(loader_override_bits).is_none()
                    && importlib_loader_is_molt_loader(_py, loader_override_bits, machinery_bits)?
                {
                    loader_override_bits = MoltObject::none().bits();
                }

                let spec_from_file_location_bits = importlib_required_callable(
                    _py,
                    util_bits,
                    runtime_static_name_slot(_py, b"spec_from_file_location"),
                    b"spec_from_file_location",
                    "importlib.util",
                )?;
                let spec_bits = call_callable_positional(
                    _py,
                    spec_from_file_location_bits,
                    &[
                        name_bits,
                        module_file_bits,
                        loader_override_bits,
                        locations_bits,
                    ],
                )?;
                if !obj_from_bits(spec_from_file_location_bits).is_none() {
                    dec_ref_bits(_py, spec_from_file_location_bits);
                }
                if !obj_from_bits(module_file_bits).is_none() {
                    dec_ref_bits(_py, module_file_bits);
                }
                if locations_owned && !obj_from_bits(locations_bits).is_none() {
                    dec_ref_bits(_py, locations_bits);
                }

                if !obj_from_bits(spec_bits).is_none() {
                    let mut loaded = false;
                    if let Some(spec_loader_bits) =
                        getattr_optional_bits(_py, spec_bits, loader_name)?
                        && !obj_from_bits(spec_loader_bits).is_none()
                    {
                        if let Some(exec_bits) = importlib_lookup_callable_attr(
                            _py,
                            spec_loader_bits,
                            runtime_static_name_slot(_py, b"exec_module"),
                            b"exec_module",
                        )? {
                            let exec_out_bits =
                                call_callable_positional(_py, exec_bits, &[module_bits])?;
                            if !obj_from_bits(exec_bits).is_none() {
                                dec_ref_bits(_py, exec_bits);
                            }
                            if !obj_from_bits(exec_out_bits).is_none() {
                                dec_ref_bits(_py, exec_out_bits);
                            }
                            importlib_dict_set_string_key(
                                _py,
                                modules_ptr,
                                name_bits,
                                module_bits,
                            )?;
                            inc_ref_bits(_py, module_bits);
                            loaded = true;
                        }
                        if !obj_from_bits(spec_loader_bits).is_none() {
                            dec_ref_bits(_py, spec_loader_bits);
                        }
                    }
                    if !obj_from_bits(spec_bits).is_none() {
                        dec_ref_bits(_py, spec_bits);
                    }
                    if loaded {
                        return Ok(module_bits);
                    }
                }
            }

            if !obj_from_bits(module_loader_bits).is_none()
                && let Some(exec_bits) = importlib_lookup_callable_attr(
                    _py,
                    module_loader_bits,
                    runtime_static_name_slot(_py, b"exec_module"),
                    b"exec_module",
                )?
            {
                let exec_out_bits = call_callable_positional(_py, exec_bits, &[module_bits])?;
                if !obj_from_bits(exec_bits).is_none() {
                    dec_ref_bits(_py, exec_bits);
                }
                if !obj_from_bits(exec_out_bits).is_none() {
                    dec_ref_bits(_py, exec_out_bits);
                }
                importlib_dict_set_string_key(_py, modules_ptr, name_bits, module_bits)?;
                inc_ref_bits(_py, module_bits);
                return Ok(module_bits);
            }

            let find_spec_bits = importlib_required_callable(
                _py,
                util_bits,
                runtime_static_name_slot(_py, b"find_spec"),
                b"find_spec",
                "importlib.util",
            )?;
            let spec_bits = call_callable_positional(
                _py,
                find_spec_bits,
                &[name_bits, MoltObject::none().bits()],
            )?;
            if !obj_from_bits(find_spec_bits).is_none() {
                dec_ref_bits(_py, find_spec_bits);
            }
            if !obj_from_bits(spec_bits).is_none() {
                if let Some(spec_loader_bits) = getattr_optional_bits(_py, spec_bits, loader_name)?
                    && !obj_from_bits(spec_loader_bits).is_none()
                {
                    if let Some(exec_bits) = importlib_lookup_callable_attr(
                        _py,
                        spec_loader_bits,
                        runtime_static_name_slot(_py, b"exec_module"),
                        b"exec_module",
                    )? {
                        let exec_out_bits =
                            call_callable_positional(_py, exec_bits, &[module_bits])?;
                        if !obj_from_bits(exec_bits).is_none() {
                            dec_ref_bits(_py, exec_bits);
                        }
                        if !obj_from_bits(exec_out_bits).is_none() {
                            dec_ref_bits(_py, exec_out_bits);
                        }
                        if !obj_from_bits(spec_loader_bits).is_none() {
                            dec_ref_bits(_py, spec_loader_bits);
                        }
                        if !obj_from_bits(spec_bits).is_none() {
                            dec_ref_bits(_py, spec_bits);
                        }
                        inc_ref_bits(_py, module_bits);
                        return Ok(module_bits);
                    }
                    if let Some(load_bits) = importlib_lookup_callable_attr(
                        _py,
                        spec_loader_bits,
                        runtime_static_name_slot(_py, b"load_module"),
                        b"load_module",
                    )? {
                        let loaded_bits = call_callable_positional(_py, load_bits, &[name_bits])?;
                        if !obj_from_bits(load_bits).is_none() {
                            dec_ref_bits(_py, load_bits);
                        }
                        if !obj_from_bits(spec_loader_bits).is_none() {
                            dec_ref_bits(_py, spec_loader_bits);
                        }
                        if !obj_from_bits(spec_bits).is_none() {
                            dec_ref_bits(_py, spec_bits);
                        }
                        return Ok(loaded_bits);
                    }
                    if !obj_from_bits(spec_loader_bits).is_none() {
                        dec_ref_bits(_py, spec_loader_bits);
                    }
                }
                if !obj_from_bits(spec_bits).is_none() {
                    dec_ref_bits(_py, spec_bits);
                }
            }

            importlib_dict_del_string_key(_py, modules_ptr, name_bits);
            call_callable_positional(_py, import_module_bits, &[name_bits])
        })();

        if module_loader_owned && !obj_from_bits(module_loader_bits).is_none() {
            dec_ref_bits(_py, module_loader_bits);
        }
        if !obj_from_bits(name_bits).is_none() {
            dec_ref_bits(_py, name_bits);
        }
        if !obj_from_bits(modules_bits).is_none() {
            dec_ref_bits(_py, modules_bits);
        }
        match out {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_find_spec_payload(
    fullname_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
    meta_path_bits: u64,
    path_hooks_bits: u64,
    path_importer_cache_bits: u64,
    package_context_bits: u64,
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
        let meta_path_count = match iterable_count_arg_from_bits(_py, meta_path_bits, "meta_path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let path_hooks_count =
            match iterable_count_arg_from_bits(_py, path_hooks_bits, "path_hooks") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let package_context = is_truthy(_py, obj_from_bits(package_context_bits));
        let via_meta_path = match importlib_find_spec_via_meta_path(
            _py,
            &fullname,
            &search_paths,
            meta_path_bits,
        ) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if let Some(spec_bits) = via_meta_path {
            return match importlib_find_spec_direct_payload_bits(
                _py,
                spec_bits,
                meta_path_count,
                path_hooks_count,
            ) {
                Ok(bits) => bits,
                Err(err) => err,
            };
        }
        // CPython only consults path hooks via meta-path finders (notably PathFinder).
        // If meta_path is empty, find_spec should not probe path_hooks directly.
        let via_path_hooks = if meta_path_count == 0 {
            None
        } else {
            match importlib_find_spec_via_path_hooks(
                _py,
                &fullname,
                &search_paths,
                path_hooks_bits,
                path_importer_cache_bits,
            ) {
                Ok(value) => value,
                Err(bits) => return bits,
            }
        };
        if let Some(spec_bits) = via_path_hooks {
            return match importlib_find_spec_direct_payload_bits(
                _py,
                spec_bits,
                meta_path_count,
                path_hooks_count,
            ) {
                Ok(bits) => bits,
                Err(err) => err,
            };
        }
        let fs_allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.find_spec.payload",
            "fs.read",
            AuditArgs::None,
            fs_allowed,
        );
        if fullname != "math" && !fs_allowed {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let payload = match importlib_find_spec_payload(
            _py,
            &fullname,
            &search_paths,
            module_file,
            meta_path_count,
            path_hooks_count,
            package_context,
        ) {
            Ok(Some(payload)) => payload,
            Ok(None) => return MoltObject::none().bits(),
            Err(bits) => return bits,
        };
        let origin_bits = match payload.origin.as_deref() {
            Some(origin) => match alloc_str_bits(_py, origin) {
                Ok(bits) => bits,
                Err(err) => return err,
            },
            None => MoltObject::none().bits(),
        };
        let locations_bits = match payload.submodule_search_locations.as_ref() {
            Some(entries) => match alloc_string_list_bits(_py, entries) {
                Some(bits) => bits,
                None => {
                    if !obj_from_bits(origin_bits).is_none() {
                        dec_ref_bits(_py, origin_bits);
                    }
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
            },
            None => MoltObject::none().bits(),
        };
        let cached_bits = match payload.cached.as_deref() {
            Some(cached) => match alloc_str_bits(_py, cached) {
                Ok(bits) => bits,
                Err(err) => {
                    if !obj_from_bits(origin_bits).is_none() {
                        dec_ref_bits(_py, origin_bits);
                    }
                    if !obj_from_bits(locations_bits).is_none() {
                        dec_ref_bits(_py, locations_bits);
                    }
                    return err;
                }
            },
            None => MoltObject::none().bits(),
        };
        let loader_kind_bits = match alloc_str_bits(_py, &payload.loader_kind) {
            Ok(bits) => bits,
            Err(err) => {
                if !obj_from_bits(origin_bits).is_none() {
                    dec_ref_bits(_py, origin_bits);
                }
                if !obj_from_bits(locations_bits).is_none() {
                    dec_ref_bits(_py, locations_bits);
                }
                if !obj_from_bits(cached_bits).is_none() {
                    dec_ref_bits(_py, cached_bits);
                }
                return err;
            }
        };
        let zip_archive_bits = match payload.zip_archive.as_deref() {
            Some(path) => match alloc_str_bits(_py, path) {
                Ok(bits) => bits,
                Err(err) => {
                    if !obj_from_bits(origin_bits).is_none() {
                        dec_ref_bits(_py, origin_bits);
                    }
                    if !obj_from_bits(locations_bits).is_none() {
                        dec_ref_bits(_py, locations_bits);
                    }
                    if !obj_from_bits(cached_bits).is_none() {
                        dec_ref_bits(_py, cached_bits);
                    }
                    dec_ref_bits(_py, loader_kind_bits);
                    return err;
                }
            },
            None => MoltObject::none().bits(),
        };
        let zip_inner_path_bits = match payload.zip_inner_path.as_deref() {
            Some(path) => match alloc_str_bits(_py, path) {
                Ok(bits) => bits,
                Err(err) => {
                    if !obj_from_bits(origin_bits).is_none() {
                        dec_ref_bits(_py, origin_bits);
                    }
                    if !obj_from_bits(locations_bits).is_none() {
                        dec_ref_bits(_py, locations_bits);
                    }
                    if !obj_from_bits(cached_bits).is_none() {
                        dec_ref_bits(_py, cached_bits);
                    }
                    dec_ref_bits(_py, loader_kind_bits);
                    if !obj_from_bits(zip_archive_bits).is_none() {
                        dec_ref_bits(_py, zip_archive_bits);
                    }
                    return err;
                }
            },
            None => MoltObject::none().bits(),
        };
        let is_package_bits = MoltObject::from_bool(payload.is_package).bits();
        let is_builtin_bits = MoltObject::from_bool(payload.is_builtin).bits();
        let has_location_bits = MoltObject::from_bool(payload.has_location).bits();
        let meta_path_count_bits = int_bits_from_i64(_py, payload.meta_path_count);
        let path_hooks_count_bits = int_bits_from_i64(_py, payload.path_hooks_count);
        let keys_and_values: [(&[u8], u64); 11] = [
            (b"origin", origin_bits),
            (b"is_package", is_package_bits),
            (b"submodule_search_locations", locations_bits),
            (b"cached", cached_bits),
            (b"is_builtin", is_builtin_bits),
            (b"has_location", has_location_bits),
            (b"loader_kind", loader_kind_bits),
            (b"zip_archive", zip_archive_bits),
            (b"zip_inner_path", zip_inner_path_bits),
            (b"meta_path_count", meta_path_count_bits),
            (b"path_hooks_count", path_hooks_count_bits),
        ];
        let mut pairs: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        let mut owned: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        for (key, value_bits) in keys_and_values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
            owned.push(value_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(dict_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_bootstrap_payload(
    search_paths_bits: u64,
    module_file_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let search_paths =
            match string_sequence_arg_from_bits(_py, search_paths_bits, "search paths") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let payload = importlib_bootstrap_payload(&search_paths, module_file);
        let resolved_paths_bits = match alloc_string_list_bits(_py, &payload.resolved_search_paths)
        {
            Some(bits) => bits,
            None => return raise_exception::<_>(_py, "MemoryError", "out of memory"),
        };
        let pythonpath_bits = match alloc_string_list_bits(_py, &payload.pythonpath_entries) {
            Some(bits) => bits,
            None => {
                dec_ref_bits(_py, resolved_paths_bits);
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
        };
        let module_roots_bits = match alloc_string_list_bits(_py, &payload.module_roots_entries) {
            Some(bits) => bits,
            None => {
                dec_ref_bits(_py, resolved_paths_bits);
                dec_ref_bits(_py, pythonpath_bits);
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
        };
        let venv_site_packages_bits =
            match alloc_string_list_bits(_py, &payload.venv_site_packages_entries) {
                Some(bits) => bits,
                None => {
                    dec_ref_bits(_py, resolved_paths_bits);
                    dec_ref_bits(_py, pythonpath_bits);
                    dec_ref_bits(_py, module_roots_bits);
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
            };
        let pwd_bits = match alloc_str_bits(_py, &payload.pwd) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, resolved_paths_bits);
                dec_ref_bits(_py, pythonpath_bits);
                dec_ref_bits(_py, module_roots_bits);
                dec_ref_bits(_py, venv_site_packages_bits);
                return err;
            }
        };
        let stdlib_root_bits = match payload.stdlib_root.as_deref() {
            Some(root) => match alloc_str_bits(_py, root) {
                Ok(bits) => bits,
                Err(err) => {
                    dec_ref_bits(_py, resolved_paths_bits);
                    dec_ref_bits(_py, pythonpath_bits);
                    dec_ref_bits(_py, module_roots_bits);
                    dec_ref_bits(_py, venv_site_packages_bits);
                    dec_ref_bits(_py, pwd_bits);
                    return err;
                }
            },
            None => MoltObject::none().bits(),
        };
        let include_cwd_bits = MoltObject::from_bool(payload.include_cwd).bits();
        let keys_and_values: [(&[u8], u64); 7] = [
            (b"resolved_search_paths", resolved_paths_bits),
            (b"pythonpath_entries", pythonpath_bits),
            (b"module_roots_entries", module_roots_bits),
            (b"venv_site_packages_entries", venv_site_packages_bits),
            (b"pwd", pwd_bits),
            (b"include_cwd", include_cwd_bits),
            (b"stdlib_root", stdlib_root_bits),
        ];
        let mut pairs: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        let mut owned: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        for (key, value_bits) in keys_and_values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
            owned.push(value_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(dict_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_runtime_state_payload() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match importlib_runtime_state_payload_bits(_py) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_runtime_modules() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match importlib_runtime_modules_bits(_py) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_runtime_state_view() -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match importlib_runtime_state_payload_bits(_py) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_existing_spec(
    module_name_bits: u64,
    modules_bits: u64,
    machinery_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };

        let Some(modules_ptr) = obj_from_bits(modules_bits).as_ptr() else {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "invalid importlib runtime state payload: modules",
            );
        };
        if unsafe { object_type_id(modules_ptr) } != TYPE_ID_DICT {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "invalid importlib runtime state payload: modules",
            );
        }

        let module_name_key_bits = match alloc_str_bits(_py, &module_name) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let existing_bits =
            match importlib_dict_get_string_key_bits(_py, modules_ptr, module_name_key_bits) {
                Ok(value) => value,
                Err(err) => {
                    if !obj_from_bits(module_name_key_bits).is_none() {
                        dec_ref_bits(_py, module_name_key_bits);
                    }
                    return err;
                }
            };
        let Some(existing_bits) = existing_bits else {
            if !obj_from_bits(module_name_key_bits).is_none() {
                dec_ref_bits(_py, module_name_key_bits);
            }
            return MoltObject::none().bits();
        };

        let spec_name = intern_runtime_static_name(_py, b"__spec__");
        if let Some(spec_bits) = match getattr_optional_bits(_py, existing_bits, spec_name) {
            Ok(value) => value,
            Err(err) => {
                if !obj_from_bits(module_name_key_bits).is_none() {
                    dec_ref_bits(_py, module_name_key_bits);
                }
                return err;
            }
        } && !obj_from_bits(spec_bits).is_none()
        {
            if !obj_from_bits(module_name_key_bits).is_none() {
                dec_ref_bits(_py, module_name_key_bits);
            }
            return spec_bits;
        }

        let file_name = intern_runtime_static_name(_py, b"__file__");
        let origin_bits = match getattr_optional_bits(_py, existing_bits, file_name) {
            Ok(Some(bits)) => {
                if string_obj_to_owned(obj_from_bits(bits)).is_some() {
                    bits
                } else {
                    if !obj_from_bits(bits).is_none() {
                        dec_ref_bits(_py, bits);
                    }
                    MoltObject::none().bits()
                }
            }
            Ok(None) => MoltObject::none().bits(),
            Err(err) => {
                if !obj_from_bits(module_name_key_bits).is_none() {
                    dec_ref_bits(_py, module_name_key_bits);
                }
                return err;
            }
        };
        let module_spec_cls_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            runtime_static_name_slot(_py, b"ModuleSpec"),
            b"ModuleSpec",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                if !obj_from_bits(origin_bits).is_none() {
                    dec_ref_bits(_py, origin_bits);
                }
                if !obj_from_bits(module_name_key_bits).is_none() {
                    dec_ref_bits(_py, module_name_key_bits);
                }
                return err;
            }
        };
        let is_package_bits = MoltObject::from_bool(false).bits();
        let out = match call_callable_positional(
            _py,
            module_spec_cls_bits,
            &[
                module_name_key_bits,
                MoltObject::none().bits(),
                origin_bits,
                is_package_bits,
            ],
        ) {
            Ok(bits) => bits,
            Err(err) => err,
        };
        if !obj_from_bits(module_spec_cls_bits).is_none() {
            dec_ref_bits(_py, module_spec_cls_bits);
        }
        if !obj_from_bits(origin_bits).is_none() {
            dec_ref_bits(_py, origin_bits);
        }
        if !obj_from_bits(module_name_key_bits).is_none() {
            dec_ref_bits(_py, module_name_key_bits);
        }
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_parent_search_paths(
    module_name_bits: u64,
    modules_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(modules_ptr) = obj_from_bits(modules_bits).as_ptr() else {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "invalid importlib runtime state payload: modules",
            );
        };
        if unsafe { object_type_id(modules_ptr) } != TYPE_ID_DICT {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "invalid importlib runtime state payload: modules",
            );
        }

        let parent_name = module_name
            .rsplit_once('.')
            .and_then(|(parent, _)| (!parent.is_empty()).then_some(parent.to_string()));

        let (has_parent, needs_parent_spec, package_context, parent_name_bits, search_paths_bits) =
            if let Some(parent_name) = parent_name {
                let parent_key_bits = match alloc_str_bits(_py, &parent_name) {
                    Ok(bits) => bits,
                    Err(err) => return err,
                };
                let parent_bits =
                    match importlib_dict_get_string_key_bits(_py, modules_ptr, parent_key_bits) {
                        Ok(value) => value,
                        Err(err) => {
                            if !obj_from_bits(parent_key_bits).is_none() {
                                dec_ref_bits(_py, parent_key_bits);
                            }
                            return err;
                        }
                    };
                if let Some(parent_bits) = parent_bits {
                    let path_name = intern_runtime_static_name(_py, b"__path__");
                    let parent_path_bits = match getattr_optional_bits(_py, parent_bits, path_name)
                    {
                        Ok(Some(bits)) => bits,
                        Ok(None) => MoltObject::none().bits(),
                        Err(err) => {
                            if !obj_from_bits(parent_key_bits).is_none() {
                                dec_ref_bits(_py, parent_key_bits);
                            }
                            return err;
                        }
                    };
                    let search_paths = match importlib_coerce_search_paths_values(
                        _py,
                        parent_path_bits,
                        "invalid parent package search path",
                    ) {
                        Ok(value) => value,
                        Err(err) => {
                            if !obj_from_bits(parent_path_bits).is_none() {
                                dec_ref_bits(_py, parent_path_bits);
                            }
                            if !obj_from_bits(parent_key_bits).is_none() {
                                dec_ref_bits(_py, parent_key_bits);
                            }
                            return err;
                        }
                    };
                    if !obj_from_bits(parent_path_bits).is_none() {
                        dec_ref_bits(_py, parent_path_bits);
                    }
                    let search_paths_bits =
                        match importlib_alloc_string_tuple_bits(_py, &search_paths) {
                            Ok(bits) => bits,
                            Err(err) => {
                                if !obj_from_bits(parent_key_bits).is_none() {
                                    dec_ref_bits(_py, parent_key_bits);
                                }
                                return err;
                            }
                        };
                    (true, false, true, parent_key_bits, search_paths_bits)
                } else {
                    let empty_tuple_ptr = alloc_tuple(_py, &[]);
                    if empty_tuple_ptr.is_null() {
                        if !obj_from_bits(parent_key_bits).is_none() {
                            dec_ref_bits(_py, parent_key_bits);
                        }
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
                    }
                    (
                        true,
                        true,
                        true,
                        parent_key_bits,
                        MoltObject::from_ptr(empty_tuple_ptr).bits(),
                    )
                }
            } else {
                (
                    false,
                    false,
                    false,
                    MoltObject::none().bits(),
                    MoltObject::none().bits(),
                )
            };

        let has_parent_bits = MoltObject::from_bool(has_parent).bits();
        let needs_parent_spec_bits = MoltObject::from_bool(needs_parent_spec).bits();
        let package_context_bits = MoltObject::from_bool(package_context).bits();

        let keys_and_values: [(&[u8], u64); 5] = [
            (b"has_parent", has_parent_bits),
            (b"parent_name", parent_name_bits),
            (b"search_paths", search_paths_bits),
            (b"needs_parent_spec", needs_parent_spec_bits),
            (b"package_context", package_context_bits),
        ];
        let mut pairs: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        let mut owned: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        for (key, value_bits) in keys_and_values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    if !obj_from_bits(bits).is_none() {
                        dec_ref_bits(_py, bits);
                    }
                }
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
            owned.push(value_bits);
        }
        let payload_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            if !obj_from_bits(bits).is_none() {
                dec_ref_bits(_py, bits);
            }
        }
        if payload_ptr.is_null() {
            raise_exception::<_>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(payload_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_ensure_default_meta_path(machinery_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let bootstrapped = &runtime_state(_py).importlib_default_meta_path_bootstrapped;

        let mark_bootstrapped = || {
            bootstrapped.store(true, Ordering::Release);
            MoltObject::none().bits()
        };

        if bootstrapped.load(Ordering::Acquire) {
            return MoltObject::none().bits();
        }

        let sys_bits = {
            let module_cache = crate::builtins::exceptions::internals::module_cache(_py);
            let guard = module_cache.lock().unwrap();
            guard.get("sys").copied()
        };
        let Some(sys_bits) = sys_bits else {
            return mark_bootstrapped();
        };
        if obj_from_bits(sys_bits).is_none() {
            return mark_bootstrapped();
        }

        let meta_path_name = intern_runtime_static_name(_py, b"meta_path");
        let Some(meta_path_bits) = (match getattr_optional_bits(_py, sys_bits, meta_path_name) {
            Ok(value) => value,
            Err(bits) => return bits,
        }) else {
            return mark_bootstrapped();
        };
        let Some(meta_path_ptr) = obj_from_bits(meta_path_bits).as_ptr() else {
            if !obj_from_bits(meta_path_bits).is_none() {
                dec_ref_bits(_py, meta_path_bits);
            }
            return mark_bootstrapped();
        };
        if unsafe { object_type_id(meta_path_ptr) } != TYPE_ID_LIST {
            if !obj_from_bits(meta_path_bits).is_none() {
                dec_ref_bits(_py, meta_path_bits);
            }
            return mark_bootstrapped();
        }
        if unsafe { !seq_vec_ref(meta_path_ptr).is_empty() } {
            if !obj_from_bits(meta_path_bits).is_none() {
                dec_ref_bits(_py, meta_path_bits);
            }
            return mark_bootstrapped();
        }

        let path_finder_name = intern_runtime_static_name(_py, b"PathFinder");
        let path_finder_bits = match getattr_optional_bits(_py, machinery_bits, path_finder_name) {
            Ok(value) => value,
            Err(bits) => {
                if !obj_from_bits(meta_path_bits).is_none() {
                    dec_ref_bits(_py, meta_path_bits);
                }
                return bits;
            }
        };
        let Some(path_finder_bits) = path_finder_bits else {
            if !obj_from_bits(meta_path_bits).is_none() {
                dec_ref_bits(_py, meta_path_bits);
            }
            return mark_bootstrapped();
        };
        if obj_from_bits(path_finder_bits).is_none() {
            if !obj_from_bits(meta_path_bits).is_none() {
                dec_ref_bits(_py, meta_path_bits);
            }
            return mark_bootstrapped();
        }

        let append_bits = crate::molt_list_append(meta_path_bits, path_finder_bits);
        if !obj_from_bits(append_bits).is_none() {
            dec_ref_bits(_py, append_bits);
        }
        if !obj_from_bits(path_finder_bits).is_none() {
            dec_ref_bits(_py, path_finder_bits);
        }
        if !obj_from_bits(meta_path_bits).is_none() {
            dec_ref_bits(_py, meta_path_bits);
        }
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        mark_bootstrapped()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_search_paths(
    search_paths_bits: u64,
    module_file_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let search_paths =
            match string_sequence_arg_from_bits(_py, search_paths_bits, "search paths") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let resolved = importlib_search_paths(&search_paths, module_file);
        match alloc_string_list_bits(_py, &resolved) {
            Some(bits) => bits,
            None => raise_exception::<_>(_py, "MemoryError", "out of memory"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_namespace_paths(
    package_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.namespace_paths",
            "fs.read",
            AuditArgs::None,
            allowed,
        );
        if !allowed {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let package = match string_arg_from_bits(_py, package_bits, "package") {
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
        let resolved = importlib_namespace_paths(&package, &search_paths, module_file);
        match alloc_string_list_bits(_py, &resolved) {
            Some(bits) => bits,
            None => raise_exception::<_>(_py, "MemoryError", "out of memory"),
        }
    })
}
