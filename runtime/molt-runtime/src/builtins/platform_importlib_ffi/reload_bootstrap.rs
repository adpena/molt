use super::*;

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_reload(
    module_bits: u64,
    util_bits: u64,
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
                    let name_attribute = intern_runtime_static_name(_py, b"name");
                    let out = getattr_optional_bits(_py, spec_bits, name_attribute);
                    if !obj_from_bits(spec_bits).is_none() {
                        dec_ref_bits(_py, spec_bits);
                    }
                    out?
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

            modules_bits = match importlib_runtime_modules_bits(_py) {
                Ok(bits) => bits,
                Err(error) => {
                    dec_ref_bits(_py, module_name_bits);
                    return Err(error);
                }
            };
            let Some(modules_ptr) = obj_from_bits(modules_bits).as_ptr() else {
                if !obj_from_bits(module_name_bits).is_none() {
                    dec_ref_bits(_py, module_name_bits);
                }
                return Err(importlib_modules_runtime_error(_py));
            };
            let in_sys_modules =
                match unsafe { dict_get_in_place(_py, modules_ptr, module_name_bits) } {
                    Ok(value) => value.is_some_and(|bits| bits == module_bits),
                    Err(()) => {
                        if !obj_from_bits(module_name_bits).is_none() {
                            dec_ref_bits(_py, module_name_bits);
                        }
                        return Err(MoltObject::none().bits());
                    }
                };
            if !in_sys_modules {
                let display = crate::object::ops_format::format_obj_str_bytes(
                    _py,
                    obj_from_bits(module_name_bits),
                );
                if !obj_from_bits(module_name_bits).is_none() {
                    dec_ref_bits(_py, module_name_bits);
                }
                if exception_pending(_py) {
                    return Err(MoltObject::none().bits());
                }
                let message = [b"module ".as_slice(), &display, b" not in sys.modules"].concat();
                return Err(crate::builtins::exceptions::raise_exception_bytes::<_>(
                    _py,
                    "ImportError",
                    &message,
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
                    && importlib_loader_is_molt_loader(_py, loader_override_bits)?
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
        let payload = importlib_bootstrap_payload(_py, &search_paths, module_file);
        let resolved_paths_bits = match alloc_string_list_bits(_py, &payload.resolved_search_paths)
        {
            Some(bits) => bits,
            None => return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel(),
        };
        let pythonpath_bits = match alloc_string_list_bits(_py, &payload.pythonpath_entries) {
            Some(bits) => bits,
            None => {
                dec_ref_bits(_py, resolved_paths_bits);
                return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
            }
        };
        let module_roots_bits = match alloc_string_list_bits(_py, &payload.module_roots_entries) {
            Some(bits) => bits,
            None => {
                dec_ref_bits(_py, resolved_paths_bits);
                dec_ref_bits(_py, pythonpath_bits);
                return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
            }
        };
        let venv_site_packages_bits =
            match alloc_string_list_bits(_py, &payload.venv_site_packages_entries) {
                Some(bits) => bits,
                None => {
                    dec_ref_bits(_py, resolved_paths_bits);
                    dec_ref_bits(_py, pythonpath_bits);
                    dec_ref_bits(_py, module_roots_bits);
                    return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
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
pub extern "C" fn molt_importlib_ensure_default_meta_path(machinery_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let bootstrapped = &runtime_state(py).importlib_default_meta_path_bootstrapped;
        if bootstrapped.load(Ordering::Acquire) {
            return MoltObject::none().bits();
        }
        let mut owned = Vec::new();
        let outcome = (|| -> Result<(), u64> {
            let Some(sys_bits) = crate::builtins::modules::interpreter_sys_module(py) else {
                return Ok(());
            };
            let meta_name = intern_runtime_static_name(py, b"meta_path");
            let Some(meta_bits) = getattr_optional_bits(py, sys_bits, meta_name)? else {
                return Ok(());
            };
            owned.push(meta_bits);
            let Some(meta_ptr) = obj_from_bits(meta_bits).as_ptr() else {
                return Ok(());
            };
            // Only the initial empty default list belongs to bootstrap. A
            // custom list/object (and later deliberate empty list) is public
            // policy, not a request to reinstall defaults.
            if unsafe {
                object_type_id(meta_ptr) != TYPE_ID_LIST
                    || crate::object::seq_access::locked_len(meta_ptr) != 0
            } {
                return Ok(());
            }
            let path_finder = importlib_required_attribute(
                py,
                machinery_bits,
                runtime_static_name_slot(py, b"PathFinder"),
                b"PathFinder",
                "importlib.machinery",
            )?;
            owned.push(path_finder);
            if obj_from_bits(path_finder).is_none() {
                return Err(raise_exception::<_>(
                    py,
                    "RuntimeError",
                    "PathFinder is unavailable",
                ));
            }
            let builtin = crate::builtins::types::compiled_loader_builtin_class(py);
            if builtin == 0 || exception_pending(py) {
                return Err(MoltObject::none().bits());
            }
            let hooks_name = intern_runtime_static_name(py, b"path_hooks");
            if let Some(hooks_bits) = getattr_optional_bits(py, sys_bits, hooks_name)? {
                owned.push(hooks_bits);
                if let Some(hooks_ptr) = obj_from_bits(hooks_bits).as_ptr()
                    && unsafe {
                        object_type_id(hooks_ptr) == TYPE_ID_LIST
                            && crate::object::seq_access::locked_len(hooks_ptr) == 0
                    }
                {
                    let Some(defaults_name) = attr_name_bits_from_bytes(py, b"_default_path_hooks")
                    else {
                        return Err(MoltObject::none().bits());
                    };
                    let defaults = getattr_optional_bits(py, machinery_bits, defaults_name);
                    dec_ref_bits(py, defaults_name);
                    let Some(defaults) = defaults? else {
                        return Err(raise_exception::<_>(
                            py,
                            "RuntimeError",
                            "default path hooks are unavailable",
                        ));
                    };
                    owned.push(defaults);
                    let Some(defaults_ptr) = obj_from_bits(defaults).as_ptr() else {
                        return Err(raise_exception::<_>(
                            py,
                            "TypeError",
                            "default path hooks must be a list",
                        ));
                    };
                    if unsafe { object_type_id(defaults_ptr) != TYPE_ID_LIST } {
                        return Err(raise_exception::<_>(
                            py,
                            "TypeError",
                            "default path hooks must be a list",
                        ));
                    }
                    let Some(snapshot) = (unsafe {
                        crate::object::seq_access::snapshot(
                            py,
                            defaults_ptr,
                            "default path hooks snapshot allocation failed",
                        )
                    }) else {
                        return Err(MoltObject::none().bits());
                    };
                    if !unsafe {
                        crate::object::list_mutation::extend_from_slice(py, hooks_ptr, &snapshot)
                    } {
                        return Err(MoltObject::none().bits());
                    }
                }
            }
            // Extend the finder list in one mutation. If allocation fails,
            // it stays empty; already-installed default hooks are retained on
            // retry instead of duplicated. No finder or hook is called here.
            if !unsafe {
                crate::object::list_mutation::extend_from_slice(
                    py,
                    meta_ptr,
                    &[builtin, path_finder],
                )
            } {
                return Err(MoltObject::none().bits());
            }
            Ok(())
        })();
        for bits in owned {
            dec_ref_bits(py, bits);
        }
        match outcome {
            Ok(()) => {
                bootstrapped.store(true, Ordering::Release);
                MoltObject::none().bits()
            }
            Err(bits) => bits,
        }
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
        let resolved = importlib_search_paths(_py, &search_paths, module_file);
        match alloc_string_list_bits(_py, &resolved) {
            Some(bits) => bits,
            None => crate::builtins::exceptions::ExceptionSentinel::exception_sentinel(),
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
        let resolved = importlib_namespace_paths(_py, &package, &search_paths, module_file);
        match alloc_string_list_bits(_py, &resolved) {
            Some(bits) => bits,
            None => crate::builtins::exceptions::ExceptionSentinel::exception_sentinel(),
        }
    })
}
