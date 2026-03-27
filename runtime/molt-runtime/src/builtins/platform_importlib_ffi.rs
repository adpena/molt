use super::*;

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_bootstrap_path(module_file_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let state = sys_bootstrap_state_from_module_file(module_file);
        alloc_string_list_bits(_py, &state.path).unwrap_or_else(|| MoltObject::none().bits())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_bootstrap_pythonpath(module_file_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let state = sys_bootstrap_state_from_module_file(module_file);
        alloc_string_list_bits(_py, &state.pythonpath_entries)
            .unwrap_or_else(|| MoltObject::none().bits())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_bootstrap_module_roots(module_file_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let state = sys_bootstrap_state_from_module_file(module_file);
        alloc_string_list_bits(_py, &state.module_roots_entries)
            .unwrap_or_else(|| MoltObject::none().bits())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_bootstrap_pwd(module_file_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let state = sys_bootstrap_state_from_module_file(module_file);
        match alloc_str_bits(_py, &state.pwd) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_bootstrap_include_cwd(module_file_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let state = sys_bootstrap_state_from_module_file(module_file);
        MoltObject::from_bool(state.include_cwd).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_bootstrap_stdlib_root(module_file_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let state = sys_bootstrap_state_from_module_file(module_file);
        match state.stdlib_root {
            Some(root) => match alloc_str_bits(_py, &root) {
                Ok(bits) => bits,
                Err(err) => err,
            },
            None => MoltObject::none().bits(),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_bootstrap_payload(module_file_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let state = sys_bootstrap_state_from_module_file(module_file);
        let path_bits = match alloc_string_list_bits(_py, &state.path) {
            Some(bits) => bits,
            None => return MoltObject::none().bits(),
        };
        let pythonpath_entries_bits = match alloc_string_list_bits(_py, &state.pythonpath_entries) {
            Some(bits) => bits,
            None => return MoltObject::none().bits(),
        };
        let module_roots_entries_bits =
            match alloc_string_list_bits(_py, &state.module_roots_entries) {
                Some(bits) => bits,
                None => return MoltObject::none().bits(),
            };
        let venv_site_packages_entries_bits =
            match alloc_string_list_bits(_py, &state.venv_site_packages_entries) {
                Some(bits) => bits,
                None => return MoltObject::none().bits(),
            };
        let pythonpath_bits = match alloc_str_bits(_py, &state.py_path_raw) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let module_roots_bits = match alloc_str_bits(_py, &state.module_roots_raw) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let virtual_env_bits = match alloc_str_bits(_py, &state.virtual_env_raw) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let dev_trusted_bits = match alloc_str_bits(_py, &state.dev_trusted_raw) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let pwd_bits = match alloc_str_bits(_py, &state.pwd) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let stdlib_root_bits = match state.stdlib_root {
            Some(root) => match alloc_str_bits(_py, &root) {
                Ok(bits) => bits,
                Err(err) => return err,
            },
            None => MoltObject::none().bits(),
        };
        let include_cwd_bits = MoltObject::from_bool(state.include_cwd).bits();

        let keys_and_values: [(&[u8], u64); 11] = [
            (b"path", path_bits),
            (b"pythonpath_entries", pythonpath_entries_bits),
            (b"module_roots_entries", module_roots_entries_bits),
            (
                b"venv_site_packages_entries",
                venv_site_packages_entries_bits,
            ),
            (b"pythonpath", pythonpath_bits),
            (b"module_roots", module_roots_bits),
            (b"virtual_env", virtual_env_bits),
            (b"dev_trusted", dev_trusted_bits),
            (b"pwd", pwd_bits),
            (b"stdlib_root", stdlib_root_bits),
            (b"include_cwd", include_cwd_bits),
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
pub extern "C" fn molt_importlib_source_loader_payload(
    module_name_bits: u64,
    path_bits: u64,
    spec_is_package_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let spec_is_package = is_truthy(_py, obj_from_bits(spec_is_package_bits));
        let resolution = source_loader_resolution(&module_name, &path, spec_is_package);
        match importlib_loader_resolution_payload_bits(_py, &resolution) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_extension_loader_payload(
    module_name_bits: u64,
    path_bits: u64,
    spec_is_package_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match importlib_path_is_file(_py, &path) {
            Ok(true) => {}
            Ok(false) => {
                return raise_exception::<_>(
                    _py,
                    "ImportError",
                    "extension module path must point to a file",
                );
            }
            Err(bits) => return bits,
        }
        if let Err(bits) = importlib_require_extension_metadata(_py, &module_name, &path) {
            return bits;
        }
        let spec_is_package = is_truthy(_py, obj_from_bits(spec_is_package_bits));
        let resolution = extension_loader_resolution(&module_name, &path, spec_is_package);
        match importlib_loader_resolution_payload_bits(_py, &resolution) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_sourceless_loader_payload(
    module_name_bits: u64,
    path_bits: u64,
    spec_is_package_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let spec_is_package = is_truthy(_py, obj_from_bits(spec_is_package_bits));
        let resolution = sourceless_loader_resolution(&module_name, &path, spec_is_package);
        match importlib_loader_resolution_payload_bits(_py, &resolution) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_source_exec_payload(
    module_name_bits: u64,
    path_bits: u64,
    spec_is_package_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let spec_is_package = is_truthy(_py, obj_from_bits(spec_is_package_bits));
        let payload = match importlib_source_exec_payload(&module_name, &path, spec_is_package) {
            Ok(value) => value,
            Err(err) => return raise_importlib_io_error(_py, err),
        };

        let source_ptr = alloc_string(_py, &payload.source);
        if source_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        let source_bits = MoltObject::from_ptr(source_ptr).bits();
        let module_package_bits = match alloc_str_bits(_py, &payload.module_package) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, source_bits);
                return err;
            }
        };
        let package_root_bits = match payload.package_root.as_deref() {
            Some(root) => match alloc_str_bits(_py, root) {
                Ok(bits) => bits,
                Err(err) => {
                    dec_ref_bits(_py, source_bits);
                    dec_ref_bits(_py, module_package_bits);
                    return err;
                }
            },
            None => MoltObject::none().bits(),
        };
        let is_package_bits = MoltObject::from_bool(payload.is_package).bits();

        let keys_and_values: [(&[u8], u64); 4] = [
            (b"source", source_bits),
            (b"is_package", is_package_bits),
            (b"module_package", module_package_bits),
            (b"package_root", package_root_bits),
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
pub extern "C" fn molt_importlib_zip_source_exec_payload(
    module_name_bits: u64,
    archive_path_bits: u64,
    inner_path_bits: u64,
    spec_is_package_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let archive_path = match string_arg_from_bits(_py, archive_path_bits, "archive path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let inner_path = match string_arg_from_bits(_py, inner_path_bits, "inner path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let spec_is_package = is_truthy(_py, obj_from_bits(spec_is_package_bits));
        let payload = match importlib_zip_source_exec_payload(
            &module_name,
            &archive_path,
            &inner_path,
            spec_is_package,
        ) {
            Ok(value) => value,
            Err(err) => return raise_importlib_io_error(_py, err),
        };

        let source_ptr = alloc_string(_py, &payload.source);
        if source_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        let source_bits = MoltObject::from_ptr(source_ptr).bits();
        let origin_bits = match alloc_str_bits(_py, &payload.origin) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, source_bits);
                return err;
            }
        };
        let module_package_bits = match alloc_str_bits(_py, &payload.module_package) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, source_bits);
                dec_ref_bits(_py, origin_bits);
                return err;
            }
        };
        let package_root_bits = match payload.package_root.as_deref() {
            Some(root) => match alloc_str_bits(_py, root) {
                Ok(bits) => bits,
                Err(err) => {
                    dec_ref_bits(_py, source_bits);
                    dec_ref_bits(_py, origin_bits);
                    dec_ref_bits(_py, module_package_bits);
                    return err;
                }
            },
            None => MoltObject::none().bits(),
        };
        let is_package_bits = MoltObject::from_bool(payload.is_package).bits();

        let keys_and_values: [(&[u8], u64); 5] = [
            (b"source", source_bits),
            (b"origin", origin_bits),
            (b"is_package", is_package_bits),
            (b"module_package", module_package_bits),
            (b"package_root", package_root_bits),
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
pub extern "C" fn molt_importlib_exec_extension(
    namespace_bits: u64,
    module_name_bits: u64,
    path_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let namespace_ptr = match obj_from_bits(namespace_bits).as_ptr() {
            Some(ptr) if unsafe { object_type_id(ptr) == TYPE_ID_DICT } => ptr,
            _ => return raise_exception::<_>(_py, "TypeError", "namespace must be dict"),
        };
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match importlib_path_is_file(_py, &path) {
            Ok(true) => {}
            Ok(false) => {
                return raise_exception::<_>(
                    _py,
                    "ImportError",
                    "extension module path must point to a file",
                );
            }
            Err(bits) => return bits,
        }
        if !has_capability(_py, "module.extension.exec") && !has_capability(_py, "module.exec") {
            return raise_exception::<_>(
                _py,
                "PermissionError",
                "missing module.extension.exec capability",
            );
        }
        if let Err(bits) = importlib_require_extension_metadata(_py, &module_name, &path) {
            return bits;
        }
        let shim_candidates = importlib_extension_shim_candidates(&module_name, &path);
        let mut restricted_error: Option<String> = None;
        for candidate in &shim_candidates {
            match importlib_path_is_file(_py, candidate) {
                Ok(true) => {
                    if let Err(err) =
                        importlib_exec_restricted_source_path(_py, namespace_ptr, candidate)
                    {
                        if let Some(message) = importlib_restricted_exec_error_message(
                            _py,
                            "extension",
                            &module_name,
                            candidate,
                        ) {
                            if restricted_error.is_none() {
                                restricted_error = Some(message);
                            }
                            continue;
                        }
                        return err;
                    }
                    return MoltObject::none().bits();
                }
                Ok(false) => continue,
                Err(bits) => return bits,
            }
        }
        if let Some(message) = restricted_error {
            return raise_exception::<_>(_py, "ImportError", &message);
        }
        // -- Native C extension loading via dlopen --
        #[cfg(all(feature = "cext_loader", not(target_arch = "wasm32")))]
        {
            match cext_loader_dlopen(_py, namespace_ptr, &module_name, &path) {
                Ok(()) => return MoltObject::none().bits(),
                Err(msg) => {
                    return raise_exception::<_>(
                        _py,
                        "ImportError",
                        &format!("failed to load C extension {module_name:?} from {path:?}: {msg}"),
                    );
                }
            }
        }
        #[allow(unreachable_code)]
        importlib_extension_exec_unavailable(
            _py,
            &module_name,
            &path,
            "extension",
            &shim_candidates,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_exec_sourceless(
    namespace_bits: u64,
    module_name_bits: u64,
    path_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let namespace_ptr = match obj_from_bits(namespace_bits).as_ptr() {
            Some(ptr) if unsafe { object_type_id(ptr) == TYPE_ID_DICT } => ptr,
            _ => return raise_exception::<_>(_py, "TypeError", "namespace must be dict"),
        };
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match importlib_path_is_file(_py, &path) {
            Ok(true) => {}
            Ok(false) => {
                return raise_exception::<_>(
                    _py,
                    "ImportError",
                    "sourceless module path must point to a file",
                );
            }
            Err(bits) => return bits,
        }
        if !has_capability(_py, "module.bytecode.exec") && !has_capability(_py, "module.exec") {
            return raise_exception::<_>(
                _py,
                "PermissionError",
                "missing module.bytecode.exec capability",
            );
        }
        let source_candidates = importlib_sourceless_source_candidates(&module_name, &path);
        let mut restricted_error: Option<String> = None;
        for candidate in &source_candidates {
            match importlib_path_is_file(_py, candidate) {
                Ok(true) => {
                    if let Err(err) =
                        importlib_exec_restricted_source_path(_py, namespace_ptr, candidate)
                    {
                        if let Some(message) = importlib_restricted_exec_error_message(
                            _py,
                            "sourceless",
                            &module_name,
                            candidate,
                        ) {
                            if restricted_error.is_none() {
                                restricted_error = Some(message);
                            }
                            continue;
                        }
                        return err;
                    }
                    return MoltObject::none().bits();
                }
                Ok(false) => continue,
                Err(bits) => return bits,
            }
        }
        if let Some(message) = restricted_error {
            return raise_exception::<_>(_py, "ImportError", &message);
        }
        importlib_extension_exec_unavailable(
            _py,
            &module_name,
            &path,
            "sourceless",
            &source_candidates,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_linecache_loader_get_source(loader_bits: u64, module_name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match linecache_loader_get_source_impl(_py, loader_bits, &module_name) {
            Ok(Some(source)) => match alloc_str_bits(_py, &source) {
                Ok(bits) => bits,
                Err(err) => err,
            },
            Ok(None) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_module_spec_is_package(module_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        match importlib_module_spec_is_package_bits(_py, module_bits) {
            Ok(value) => MoltObject::from_bool(value).bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_coerce_module_name(
    module_bits: u64,
    loader_bits: u64,
    spec_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        static MODULE_NAME_NAME: AtomicU64 = AtomicU64::new(0);
        static SPEC_NAME: AtomicU64 = AtomicU64::new(0);
        static LOADER_NAME: AtomicU64 = AtomicU64::new(0);

        let module_name_name = intern_static_name(_py, &MODULE_NAME_NAME, b"__name__");
        if let Some(module_name_bits) =
            match getattr_optional_bits(_py, module_bits, module_name_name) {
                Ok(value) => value,
                Err(bits) => return bits,
            }
        {
            if string_obj_to_owned(obj_from_bits(module_name_bits)).is_some() {
                return module_name_bits;
            }
            if !obj_from_bits(module_name_bits).is_none() {
                dec_ref_bits(_py, module_name_bits);
            }
        }

        let mut module_spec_bits = spec_bits;
        let mut module_spec_owned = false;
        if obj_from_bits(module_spec_bits).is_none() {
            let spec_name = intern_static_name(_py, &SPEC_NAME, b"__spec__");
            if let Some(bits) = match getattr_optional_bits(_py, module_bits, spec_name) {
                Ok(value) => value,
                Err(err) => return err,
            } {
                module_spec_bits = bits;
                module_spec_owned = true;
            }
        }

        if !obj_from_bits(module_spec_bits).is_none()
            && let Some(spec_name_bits) =
                match getattr_optional_bits(_py, module_spec_bits, module_name_name) {
                    Ok(value) => value,
                    Err(bits) => {
                        if module_spec_owned {
                            dec_ref_bits(_py, module_spec_bits);
                        }
                        return bits;
                    }
                }
        {
            if string_obj_to_owned(obj_from_bits(spec_name_bits)).is_some() {
                let set_bits =
                    crate::molt_object_setattr(module_bits, module_name_name, spec_name_bits);
                if !obj_from_bits(set_bits).is_none() {
                    dec_ref_bits(_py, set_bits);
                }
                if exception_pending(_py) {
                    clear_exception(_py);
                }
                if module_spec_owned {
                    dec_ref_bits(_py, module_spec_bits);
                }
                return spec_name_bits;
            }
            if !obj_from_bits(spec_name_bits).is_none() {
                dec_ref_bits(_py, spec_name_bits);
            }
        }

        if module_spec_owned && !obj_from_bits(module_spec_bits).is_none() {
            dec_ref_bits(_py, module_spec_bits);
        }

        let loader_name = intern_static_name(_py, &LOADER_NAME, b"name");
        if let Some(loader_name_bits) = match getattr_optional_bits(_py, loader_bits, loader_name) {
            Ok(value) => value,
            Err(bits) => return bits,
        } {
            if string_obj_to_owned(obj_from_bits(loader_name_bits)).is_some() {
                let set_bits =
                    crate::molt_object_setattr(module_bits, module_name_name, loader_name_bits);
                if !obj_from_bits(set_bits).is_none() {
                    dec_ref_bits(_py, set_bits);
                }
                if exception_pending(_py) {
                    clear_exception(_py);
                }
                return loader_name_bits;
            }
            if !obj_from_bits(loader_name_bits).is_none() {
                dec_ref_bits(_py, loader_name_bits);
            }
        }

        raise_exception::<_>(_py, "TypeError", "module name must be str")
    })
}

pub(super) fn importlib_coerce_search_paths_values(
    _py: &PyToken<'_>,
    value_bits: u64,
    label: &str,
) -> Result<Vec<String>, u64> {
    let mut paths: Vec<String> = Vec::new();
    if obj_from_bits(value_bits).is_none() {
        // value is None -> ()
    } else if let Some(text) = string_obj_to_owned(obj_from_bits(value_bits)) {
        if !text.is_empty() {
            paths.push(text);
        }
    } else {
        let iter_bits = molt_iter(value_bits);
        if exception_pending(_py) {
            clear_exception(_py);
            return Err(raise_exception::<_>(_py, "RuntimeError", label));
        }
        loop {
            let pair_bits = molt_iter_next(iter_bits);
            let Some(pair_ptr) = maybe_ptr_from_bits(pair_bits) else {
                clear_exception(_py);
                return Err(raise_exception::<_>(_py, "RuntimeError", label));
            };
            let pair = unsafe {
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    clear_exception(_py);
                    return Err(raise_exception::<_>(_py, "RuntimeError", label));
                }
                seq_vec_ref(pair_ptr)
            };
            if pair.len() < 2 {
                clear_exception(_py);
                return Err(raise_exception::<_>(_py, "RuntimeError", label));
            }
            if is_truthy(_py, obj_from_bits(pair[1])) {
                break;
            }
            let text_bits = unsafe { call_callable1(_py, builtin_classes(_py).str, pair[0]) };
            if exception_pending(_py) {
                clear_exception(_py);
                if !obj_from_bits(text_bits).is_none() {
                    dec_ref_bits(_py, text_bits);
                }
                return Err(raise_exception::<_>(_py, "RuntimeError", label));
            }
            let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
                if !obj_from_bits(text_bits).is_none() {
                    dec_ref_bits(_py, text_bits);
                }
                clear_exception(_py);
                return Err(raise_exception::<_>(_py, "RuntimeError", label));
            };
            if !obj_from_bits(text_bits).is_none() {
                dec_ref_bits(_py, text_bits);
            }
            if !text.is_empty() {
                paths.push(text);
            }
        }
    }
    Ok(paths)
}

fn importlib_alloc_string_tuple_bits(_py: &PyToken<'_>, values: &[String]) -> Result<u64, u64> {
    let mut value_bits_vec: Vec<u64> = Vec::with_capacity(values.len());
    for value in values {
        let bits = match alloc_str_bits(_py, value) {
            Ok(bits) => bits,
            Err(err) => {
                for owned in value_bits_vec {
                    if !obj_from_bits(owned).is_none() {
                        dec_ref_bits(_py, owned);
                    }
                }
                return Err(err);
            }
        };
        value_bits_vec.push(bits);
    }
    let tuple_ptr = alloc_tuple(_py, &value_bits_vec);
    for owned in value_bits_vec {
        if !obj_from_bits(owned).is_none() {
            dec_ref_bits(_py, owned);
        }
    }
    if tuple_ptr.is_null() {
        Err(raise_exception::<_>(_py, "MemoryError", "out of memory"))
    } else {
        Ok(MoltObject::from_ptr(tuple_ptr).bits())
    }
}

fn importlib_finder_signature_tuple_bits(
    _py: &PyToken<'_>,
    finders_bits: u64,
    label: &str,
) -> Result<u64, u64> {
    let mut ids: Vec<u64> = Vec::new();
    let iter_bits = molt_iter(finders_bits);
    if exception_pending(_py) {
        clear_exception(_py);
        return Err(raise_exception::<_>(_py, "RuntimeError", label));
    }
    loop {
        let pair_bits = molt_iter_next(iter_bits);
        let Some(pair_ptr) = maybe_ptr_from_bits(pair_bits) else {
            clear_exception(_py);
            return Err(raise_exception::<_>(_py, "RuntimeError", label));
        };
        let pair = unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                clear_exception(_py);
                return Err(raise_exception::<_>(_py, "RuntimeError", label));
            }
            seq_vec_ref(pair_ptr)
        };
        if pair.len() < 2 {
            clear_exception(_py);
            return Err(raise_exception::<_>(_py, "RuntimeError", label));
        }
        if is_truthy(_py, obj_from_bits(pair[1])) {
            break;
        }
        let id_bits = crate::molt_id(pair[0]);
        if exception_pending(_py) {
            clear_exception(_py);
            if !obj_from_bits(id_bits).is_none() {
                dec_ref_bits(_py, id_bits);
            }
            for owned in ids {
                if !obj_from_bits(owned).is_none() {
                    dec_ref_bits(_py, owned);
                }
            }
            return Err(raise_exception::<_>(_py, "RuntimeError", label));
        }
        ids.push(id_bits);
    }
    let tuple_ptr = alloc_tuple(_py, &ids);
    for owned in ids {
        if !obj_from_bits(owned).is_none() {
            dec_ref_bits(_py, owned);
        }
    }
    if tuple_ptr.is_null() {
        Err(raise_exception::<_>(_py, "MemoryError", "out of memory"))
    } else {
        Ok(MoltObject::from_ptr(tuple_ptr).bits())
    }
}

fn importlib_path_importer_cache_signature_tuple_bits(
    _py: &PyToken<'_>,
    path_importer_cache_bits: u64,
    label: &str,
) -> Result<u64, u64> {
    if obj_from_bits(path_importer_cache_bits).is_none() {
        let empty_ptr = alloc_tuple(_py, &[]);
        if empty_ptr.is_null() {
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        return Ok(MoltObject::from_ptr(empty_ptr).bits());
    }

    let Some(cache_ptr) = obj_from_bits(path_importer_cache_bits).as_ptr() else {
        return Err(raise_exception::<_>(_py, "RuntimeError", label));
    };
    if unsafe { object_type_id(cache_ptr) } != TYPE_ID_DICT {
        return Err(raise_exception::<_>(_py, "RuntimeError", label));
    }

    let mut pairs: Vec<(String, u64)> = Vec::new();
    let entries = unsafe { dict_order(cache_ptr) };
    for idx in (0..entries.len()).step_by(2) {
        let key_bits = entries[idx];
        let value_bits = entries[idx + 1];
        let Some(key) = string_obj_to_owned(obj_from_bits(key_bits)) else {
            continue;
        };
        let id_bits = crate::molt_id(value_bits);
        if exception_pending(_py) {
            clear_exception(_py);
            if !obj_from_bits(id_bits).is_none() {
                dec_ref_bits(_py, id_bits);
            }
            for (_k, owned) in pairs {
                if !obj_from_bits(owned).is_none() {
                    dec_ref_bits(_py, owned);
                }
            }
            return Err(raise_exception::<_>(_py, "RuntimeError", label));
        }
        pairs.push((key, id_bits));
    }
    pairs.sort_by(|lhs, rhs| lhs.0.cmp(&rhs.0));

    let mut pair_tuple_bits: Vec<u64> = Vec::with_capacity(pairs.len());
    for (key, id_bits) in pairs {
        let key_bits = match alloc_str_bits(_py, &key) {
            Ok(bits) => bits,
            Err(err) => {
                if !obj_from_bits(id_bits).is_none() {
                    dec_ref_bits(_py, id_bits);
                }
                for owned in pair_tuple_bits {
                    if !obj_from_bits(owned).is_none() {
                        dec_ref_bits(_py, owned);
                    }
                }
                return Err(err);
            }
        };
        let item_ptr = alloc_tuple(_py, &[key_bits, id_bits]);
        if !obj_from_bits(key_bits).is_none() {
            dec_ref_bits(_py, key_bits);
        }
        if !obj_from_bits(id_bits).is_none() {
            dec_ref_bits(_py, id_bits);
        }
        if item_ptr.is_null() {
            for owned in pair_tuple_bits {
                if !obj_from_bits(owned).is_none() {
                    dec_ref_bits(_py, owned);
                }
            }
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        pair_tuple_bits.push(MoltObject::from_ptr(item_ptr).bits());
    }
    let out_ptr = alloc_tuple(_py, &pair_tuple_bits);
    for owned in pair_tuple_bits {
        if !obj_from_bits(owned).is_none() {
            dec_ref_bits(_py, owned);
        }
    }
    if out_ptr.is_null() {
        Err(raise_exception::<_>(_py, "MemoryError", "out of memory"))
    } else {
        Ok(MoltObject::from_ptr(out_ptr).bits())
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_coerce_search_paths(value_bits: u64, label_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let label = match string_arg_from_bits(_py, label_bits, "label") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let paths = match importlib_coerce_search_paths_values(_py, value_bits, &label) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match importlib_alloc_string_tuple_bits(_py, &paths) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_finder_signature(finders_bits: u64, label_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let label = match string_arg_from_bits(_py, label_bits, "label") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match importlib_finder_signature_tuple_bits(_py, finders_bits, &label) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_path_importer_cache_signature(
    path_importer_cache_bits: u64,
    label_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let label = match string_arg_from_bits(_py, label_bits, "label") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match importlib_path_importer_cache_signature_tuple_bits(
            _py,
            path_importer_cache_bits,
            &label,
        ) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_path_is_archive_member(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(importlib_is_archive_member_path(&path)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_package_root_from_origin(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match importlib_package_root_from_origin(&path) {
            Some(root) => match alloc_str_bits(_py, &root) {
                Ok(bits) => bits,
                Err(err) => err,
            },
            None => MoltObject::none().bits(),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_validate_resource_name(resource_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let resource = match string_arg_from_bits(_py, resource_bits, "resource name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if let Err(err) = importlib_validate_resource_name_text(_py, &resource) {
            return err;
        }
        match alloc_str_bits(_py, &resource) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_normalize_path(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let text_bits = unsafe { call_callable1(_py, builtin_classes(_py).str, path_bits) };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let Some(path) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            if !obj_from_bits(text_bits).is_none() {
                dec_ref_bits(_py, text_bits);
            }
            return raise_exception::<_>(_py, "TypeError", "path must be str-like");
        };
        if !obj_from_bits(text_bits).is_none() {
            dec_ref_bits(_py, text_bits);
        }
        if let Err(err) = importlib_validate_resource_name_text(_py, &path) {
            return err;
        }
        match alloc_str_bits(_py, &path) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_only(
    iterable_bits: u64,
    default_bits: u64,
    too_long_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let iter_bits = molt_iter(iterable_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let Some(first_bits) = (match importlib_iter_next_value_bits(_py, iter_bits) {
            Ok(value) => value,
            Err(err) => return err,
        }) else {
            inc_ref_bits(_py, default_bits);
            return default_bits;
        };
        let second_value = match importlib_iter_next_value_bits(_py, iter_bits) {
            Ok(value) => value,
            Err(err) => {
                if !obj_from_bits(first_bits).is_none() {
                    dec_ref_bits(_py, first_bits);
                }
                return err;
            }
        };
        let Some(second_bits) = second_value else {
            return first_bits;
        };
        let first_text = importlib_best_effort_str(_py, first_bits);
        let second_text = importlib_best_effort_str(_py, second_bits);
        if !obj_from_bits(first_bits).is_none() {
            dec_ref_bits(_py, first_bits);
        }
        if !obj_from_bits(second_bits).is_none() {
            dec_ref_bits(_py, second_bits);
        }
        let message = format!(
            "Expected exactly one item in iterable, but got {:?}, {:?}, and perhaps more.",
            first_text, second_text
        );
        if !obj_from_bits(too_long_bits).is_none()
            && let Some(kind) = importlib_exception_name_from_bits(_py, too_long_bits)
        {
            return raise_exception::<_>(_py, &kind, &message);
        }
        raise_exception::<_>(_py, "ValueError", &message)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_reader_resource_path_from_roots(
    roots_bits: u64,
    resource_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let roots = match string_sequence_arg_from_bits(_py, roots_bits, "roots") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let resource = match string_arg_from_bits(_py, resource_bits, "resource name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if let Err(err) = importlib_validate_resource_name_text(_py, &resource) {
            return err;
        }
        if let Some(candidate) = importlib_resources_first_fs_file_candidate(&roots, &resource) {
            return match alloc_str_bits(_py, &candidate) {
                Ok(bits) => bits,
                Err(err) => err,
            };
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_reader_open_resource_bytes_from_roots(
    roots_bits: u64,
    resource_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let roots = match string_sequence_arg_from_bits(_py, roots_bits, "roots") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let resource = match string_arg_from_bits(_py, resource_bits, "resource name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if let Err(err) = importlib_validate_resource_name_text(_py, &resource) {
            return err;
        }
        if let Some(candidate) = importlib_resources_first_file_candidate(&roots, &resource) {
            let bytes = match importlib_read_file_bytes(_py, &candidate) {
                Ok(value) => value,
                Err(err) => return err,
            };
            let out_ptr = alloc_bytes(_py, &bytes);
            if out_ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            return MoltObject::from_ptr(out_ptr).bits();
        }
        raise_exception::<_>(_py, "FileNotFoundError", &resource)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_reader_is_resource_from_roots(
    roots_bits: u64,
    resource_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let roots = match string_sequence_arg_from_bits(_py, roots_bits, "roots") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let resource = match string_arg_from_bits(_py, resource_bits, "resource name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if let Err(err) = importlib_validate_resource_name_text(_py, &resource) {
            return err;
        }
        MoltObject::from_bool(importlib_resources_first_file_candidate(&roots, &resource).is_some())
            .bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_reader_contents_from_roots(roots_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let roots = match string_sequence_arg_from_bits(_py, roots_bits, "roots") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut entries: BTreeSet<String> = BTreeSet::new();
        for root in roots {
            let payload = importlib_resources_path_payload(&root);
            for entry in payload.entries {
                entries.insert(entry);
            }
        }
        let out: Vec<String> = entries.into_iter().collect();
        match alloc_string_list_bits(_py, &out) {
            Some(bits) => bits,
            None => raise_exception::<_>(_py, "MemoryError", "out of memory"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_traceback_exception_suppress_context(value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        match traceback_exception_suppress_context_bits(_py, value_bits) {
            Ok(value) => MoltObject::from_bool(value).bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_zip_read_entry(
    archive_path_bits: u64,
    inner_path_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let archive_path = match string_arg_from_bits(_py, archive_path_bits, "archive path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let inner_path = match string_arg_from_bits(_py, inner_path_bits, "inner path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let normalized = inner_path.replace('\\', "/").trim_matches('/').to_string();
        if normalized.is_empty() {
            return raise_exception::<_>(_py, "OSError", "zip archive entry path is empty");
        }
        let bytes = match zip_archive_read_entry(&archive_path, &normalized) {
            Ok(value) => value,
            Err(err) => return raise_importlib_io_error(_py, err),
        };
        let out_ptr = alloc_bytes(_py, &bytes);
        if out_ptr.is_null() {
            raise_exception::<_>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_read_file(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let bytes = match importlib_read_file_bytes(_py, &path) {
            Ok(bytes) => bytes,
            Err(bits) => return bits,
        };
        let out_ptr = alloc_bytes(_py, &bytes);
        if out_ptr.is_null() {
            raise_exception::<_>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_cache_from_source(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let cached = importlib_cache_from_source(&path);
        match alloc_str_bits(_py, &cached) {
            Ok(bits) => bits,
            Err(err_bits) => err_bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_decode_source(source_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(source_ptr) = obj_from_bits(source_bits).as_ptr() else {
            inc_ref_bits(_py, source_bits);
            return source_bits;
        };
        let source_type = unsafe { object_type_id(source_ptr) };
        if source_type != TYPE_ID_BYTES && source_type != TYPE_ID_BYTEARRAY {
            inc_ref_bits(_py, source_bits);
            return source_bits;
        }
        let encoding_bits = match alloc_str_bits(_py, "utf-8") {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let decoded_bits = if source_type == TYPE_ID_BYTES {
            molt_bytes_decode(source_bits, encoding_bits, MoltObject::none().bits())
        } else {
            molt_bytearray_decode(source_bits, encoding_bits, MoltObject::none().bits())
        };
        dec_ref_bits(_py, encoding_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        decoded_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_source_hash(source_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let source = match bytes_arg_from_bits(_py, source_bits, "source_bytes") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut hasher = Sha1::new();
        hasher.update(&source);
        let digest = hasher.finalize();
        let out_ptr = alloc_bytes(_py, &digest[..8]);
        if out_ptr.is_null() {
            raise_exception::<_>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_source_from_cache(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        inc_ref_bits(_py, path_bits);
        path_bits
    })
}

fn importlib_find_in_path_payload(
    _py: &PyToken<'_>,
    fullname_bits: u64,
    search_paths_bits: u64,
    package_context: bool,
) -> u64 {
    if !has_capability(_py, "fs.read") {
        return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
    }
    let fullname = match string_arg_from_bits(_py, fullname_bits, "module name") {
        Ok(value) => value,
        Err(bits) => return bits,
    };
    let search_paths = match string_sequence_arg_from_bits(_py, search_paths_bits, "search paths") {
        Ok(value) => value,
        Err(bits) => return bits,
    };
    let Some(resolution) = importlib_find_in_path(&fullname, &search_paths, package_context) else {
        return MoltObject::none().bits();
    };
    let origin_bits = match resolution.origin.as_deref() {
        Some(origin) => match alloc_str_bits(_py, origin) {
            Ok(bits) => bits,
            Err(err_bits) => return err_bits,
        },
        None => MoltObject::none().bits(),
    };
    let locations_bits = match resolution.submodule_search_locations.as_ref() {
        Some(entries) => match alloc_string_list_bits(_py, entries) {
            Some(bits) => bits,
            None => {
                dec_ref_bits(_py, origin_bits);
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
        },
        None => MoltObject::none().bits(),
    };
    let cached_bits = match resolution.cached.as_deref() {
        Some(cached) => match alloc_str_bits(_py, cached) {
            Ok(bits) => bits,
            Err(err_bits) => {
                if !obj_from_bits(origin_bits).is_none() {
                    dec_ref_bits(_py, origin_bits);
                }
                if !obj_from_bits(locations_bits).is_none() {
                    dec_ref_bits(_py, locations_bits);
                }
                return err_bits;
            }
        },
        None => MoltObject::none().bits(),
    };
    let loader_kind_bits = match alloc_str_bits(_py, &resolution.loader_kind) {
        Ok(bits) => bits,
        Err(err_bits) => {
            if !obj_from_bits(origin_bits).is_none() {
                dec_ref_bits(_py, origin_bits);
            }
            if !obj_from_bits(locations_bits).is_none() {
                dec_ref_bits(_py, locations_bits);
            }
            if !obj_from_bits(cached_bits).is_none() {
                dec_ref_bits(_py, cached_bits);
            }
            return err_bits;
        }
    };
    let zip_archive_bits = match resolution.zip_archive.as_deref() {
        Some(path) => match alloc_str_bits(_py, path) {
            Ok(bits) => bits,
            Err(err_bits) => {
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
                return err_bits;
            }
        },
        None => MoltObject::none().bits(),
    };
    let zip_inner_path_bits = match resolution.zip_inner_path.as_deref() {
        Some(path) => match alloc_str_bits(_py, path) {
            Ok(bits) => bits,
            Err(err_bits) => {
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
                return err_bits;
            }
        },
        None => MoltObject::none().bits(),
    };
    let is_package_bits = MoltObject::from_bool(resolution.is_package).bits();
    let has_location_bits = MoltObject::from_bool(resolution.has_location).bits();
    let keys_and_values: [(&[u8], u64); 8] = [
        (b"origin", origin_bits),
        (b"is_package", is_package_bits),
        (b"submodule_search_locations", locations_bits),
        (b"cached", cached_bits),
        (b"loader_kind", loader_kind_bits),
        (b"has_location", has_location_bits),
        (b"zip_archive", zip_archive_bits),
        (b"zip_inner_path", zip_inner_path_bits),
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
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resolve_name(name_bits: u64, package_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name = match string_arg_from_bits(_py, name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if !name.starts_with('.') {
            return match alloc_str_bits(_py, &name) {
                Ok(bits) => bits,
                Err(err) => err,
            };
        }

        let package = match optional_string_arg_from_bits(_py, package_bits, "package") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(package_name) = package.filter(|value| !value.is_empty()) else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                &format!(
                    "the 'package' argument is required to perform a relative import for '{name}'"
                ),
            );
        };

        let level = name
            .as_bytes()
            .iter()
            .take_while(|&&byte| byte == b'.')
            .count();
        if level == 0 {
            return match alloc_str_bits(_py, &name) {
                Ok(bits) => bits,
                Err(err) => err,
            };
        }

        let package_bits: Vec<&str> = package_name.split('.').collect();
        if level > package_bits.len() {
            return raise_exception::<_>(
                _py,
                "ImportError",
                "attempted relative import beyond top-level package",
            );
        }
        let base = package_bits[..(package_bits.len() - level)].join(".");
        let suffix = &name[level..];
        let resolved = if base.is_empty() {
            suffix.to_string()
        } else {
            format!("{base}{suffix}")
        };
        match alloc_str_bits(_py, &resolved) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_known_absent_missing_name(resolved_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let resolved = match string_arg_from_bits(_py, resolved_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(name) = importlib_known_absent_missing_name(_py, &resolved) else {
            return MoltObject::none().bits();
        };
        match alloc_str_bits(_py, &name) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_import_optional(module_name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let name_bits = match alloc_str_bits(_py, &module_name) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let imported_bits = crate::molt_module_import(name_bits);
        if !obj_from_bits(name_bits).is_none() {
            dec_ref_bits(_py, name_bits);
        }
        if !exception_pending(_py) {
            return imported_bits;
        }
        // Optional imports mirror `try: import x; except ImportError: ...`.
        // They should not propagate module absence and simply yield None.
        if clear_pending_if_kind(_py, &["ImportError", "ModuleNotFoundError"]) {
            if !obj_from_bits(imported_bits).is_none() {
                dec_ref_bits(_py, imported_bits);
            }
            return MoltObject::none().bits();
        }
        if !obj_from_bits(imported_bits).is_none() {
            dec_ref_bits(_py, imported_bits);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_import_or_fallback(
    module_name_bits: u64,
    fallback_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let name_bits = match alloc_str_bits(_py, &module_name) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let imported_bits = crate::molt_module_import(name_bits);
        if !obj_from_bits(name_bits).is_none() {
            dec_ref_bits(_py, name_bits);
        }
        if !exception_pending(_py) {
            return imported_bits;
        }
        if clear_pending_if_kind(_py, &["ImportError", "ModuleNotFoundError"]) {
            if !obj_from_bits(imported_bits).is_none() {
                dec_ref_bits(_py, imported_bits);
            }
            inc_ref_bits(_py, fallback_bits);
            return fallback_bits;
        }
        if !obj_from_bits(imported_bits).is_none() {
            dec_ref_bits(_py, imported_bits);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_import_required(module_name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let name_bits = match alloc_str_bits(_py, &module_name) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let imported_bits = crate::molt_module_import(name_bits);
        if !obj_from_bits(name_bits).is_none() {
            dec_ref_bits(_py, name_bits);
        }
        if exception_pending(_py) {
            if !obj_from_bits(imported_bits).is_none() {
                dec_ref_bits(_py, imported_bits);
            }
            return MoltObject::none().bits();
        }
        imported_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_export_attrs(
    module_name_bits: u64,
    export_names_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let export_names =
            match string_sequence_arg_from_bits(_py, export_names_bits, "export names") {
                Ok(values) => values,
                Err(err) => return err,
            };

        let name_bits = match alloc_str_bits(_py, &module_name) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let module_bits = crate::molt_module_import(name_bits);
        if !obj_from_bits(name_bits).is_none() {
            dec_ref_bits(_py, name_bits);
        }
        if exception_pending(_py) {
            if !obj_from_bits(module_bits).is_none() {
                dec_ref_bits(_py, module_bits);
            }
            return MoltObject::none().bits();
        }

        let mut pairs: Vec<u64> = Vec::with_capacity(export_names.len() * 2);
        let mut owned: Vec<u64> = Vec::with_capacity(export_names.len() * 2);
        let missing = missing_bits(_py);
        for export_name in export_names {
            let attr_name_bits = match alloc_str_bits(_py, &export_name) {
                Ok(bits) => bits,
                Err(err) => {
                    for bits in owned {
                        dec_ref_bits(_py, bits);
                    }
                    if !obj_from_bits(module_bits).is_none() {
                        dec_ref_bits(_py, module_bits);
                    }
                    return err;
                }
            };
            let value_bits = molt_getattr_builtin(module_bits, attr_name_bits, missing);
            if !obj_from_bits(attr_name_bits).is_none() {
                dec_ref_bits(_py, attr_name_bits);
            }
            if exception_pending(_py) {
                if !obj_from_bits(value_bits).is_none() {
                    dec_ref_bits(_py, value_bits);
                }
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                if !obj_from_bits(module_bits).is_none() {
                    dec_ref_bits(_py, module_bits);
                }
                return MoltObject::none().bits();
            }
            if is_missing_bits(_py, value_bits) {
                if !obj_from_bits(value_bits).is_none() {
                    dec_ref_bits(_py, value_bits);
                }
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                if !obj_from_bits(module_bits).is_none() {
                    dec_ref_bits(_py, module_bits);
                }
                return raise_exception::<_>(
                    _py,
                    "AttributeError",
                    &format!("module '{module_name}' has no attribute '{export_name}'"),
                );
            }

            let key_bits = match alloc_str_bits(_py, &export_name) {
                Ok(bits) => bits,
                Err(err) => {
                    if !obj_from_bits(value_bits).is_none() {
                        dec_ref_bits(_py, value_bits);
                    }
                    for bits in owned {
                        dec_ref_bits(_py, bits);
                    }
                    if !obj_from_bits(module_bits).is_none() {
                        dec_ref_bits(_py, module_bits);
                    }
                    return err;
                }
            };
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
            owned.push(value_bits);
        }

        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if !obj_from_bits(module_bits).is_none() {
            dec_ref_bits(_py, module_bits);
        }
        if dict_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_load_module_shim(
    bootstrap_bits: u64,
    loader_bits: u64,
    fullname_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        static LOAD_MODULE_SHIM_NAME: AtomicU64 = AtomicU64::new(0);

        let fullname = match string_arg_from_bits(_py, fullname_bits, "fullname") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let call_bits = match importlib_required_callable(
            _py,
            bootstrap_bits,
            &LOAD_MODULE_SHIM_NAME,
            b"_load_module_shim",
            "importlib._bootstrap",
        ) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let fullname_arg_bits = match alloc_str_bits(_py, &fullname) {
            Ok(bits) => bits,
            Err(err) => {
                if !obj_from_bits(call_bits).is_none() {
                    dec_ref_bits(_py, call_bits);
                }
                return err;
            }
        };
        let out = unsafe { call_callable2(_py, call_bits, loader_bits, fullname_arg_bits) };
        if !obj_from_bits(fullname_arg_bits).is_none() {
            dec_ref_bits(_py, fullname_arg_bits);
        }
        if !obj_from_bits(call_bits).is_none() {
            dec_ref_bits(_py, call_bits);
        }
        if exception_pending(_py) {
            if !obj_from_bits(out).is_none() {
                dec_ref_bits(_py, out);
            }
            return MoltObject::none().bits();
        }
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_joinpath(traversable_bits: u64, child_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        static JOINPATH_NAME: AtomicU64 = AtomicU64::new(0);

        let joinpath_name = intern_static_name(_py, &JOINPATH_NAME, b"joinpath");
        let missing = missing_bits(_py);
        let call_bits = molt_getattr_builtin(traversable_bits, joinpath_name, missing);
        if exception_pending(_py) {
            if !obj_from_bits(call_bits).is_none() {
                dec_ref_bits(_py, call_bits);
            }
            return MoltObject::none().bits();
        }
        if is_missing_bits(_py, call_bits) {
            if !obj_from_bits(call_bits).is_none() {
                dec_ref_bits(_py, call_bits);
            }
            return raise_exception::<_>(
                _py,
                "AttributeError",
                "traversable has no attribute 'joinpath'",
            );
        }
        let out = unsafe { call_callable1(_py, call_bits, child_bits) };
        if !obj_from_bits(call_bits).is_none() {
            dec_ref_bits(_py, call_bits);
        }
        if exception_pending(_py) {
            if !obj_from_bits(out).is_none() {
                dec_ref_bits(_py, out);
            }
            return MoltObject::none().bits();
        }
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_open_mode_is_text(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mode = match string_arg_from_bits(_py, mode_bits, "mode") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if mode == "r" {
            return MoltObject::from_bool(true).bits();
        }
        if mode == "rb" {
            return MoltObject::from_bool(false).bits();
        }
        raise_exception::<_>(
            _py,
            "ValueError",
            &format!("Invalid mode value {mode:?}, only 'r' and 'rb' are supported"),
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_package_leaf_name(package_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let package = match string_arg_from_bits(_py, package_bits, "package") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let leaf = package
            .rsplit_once('.')
            .map(|(_, tail)| tail)
            .unwrap_or(package.as_str());
        let leaf = if leaf.is_empty() {
            package.as_str()
        } else {
            leaf
        };
        match alloc_str_bits(_py, leaf) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_import_module(
    resolved_bits: u64,
    util_bits: u64,
    machinery_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let resolved = match string_arg_from_bits(_py, resolved_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let modules_bits = match importlib_runtime_modules_bits(_py) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let Some(modules_ptr) = obj_from_bits(modules_bits).as_ptr() else {
            if !obj_from_bits(modules_bits).is_none() {
                dec_ref_bits(_py, modules_bits);
            }
            return importlib_modules_runtime_error(_py);
        };
        let resolved_key_bits = match alloc_str_bits(_py, &resolved) {
            Ok(bits) => bits,
            Err(err) => {
                if !obj_from_bits(modules_bits).is_none() {
                    dec_ref_bits(_py, modules_bits);
                }
                return err;
            }
        };
        let out = (|| -> u64 {
            let cached_bits =
                match importlib_dict_get_string_key_bits(_py, modules_ptr, resolved_key_bits) {
                    Ok(bits) => bits,
                    Err(err) => return err,
                };
            if let Some(cached_bits) = cached_bits {
                let is_empty =
                    match importlib_module_is_empty_placeholder(_py, &resolved, cached_bits) {
                        Ok(value) => value,
                        Err(err) => return err,
                    };
                let should_retry =
                    match importlib_module_should_retry_empty(_py, &resolved, cached_bits) {
                        Ok(value) => value,
                        Err(err) => return err,
                    };
                if !is_empty && !should_retry {
                    if let Err(err) =
                        importlib_bind_submodule_on_parent(_py, &resolved, cached_bits, modules_ptr)
                    {
                        return err;
                    }
                    inc_ref_bits(_py, cached_bits);
                    return cached_bits;
                }
                importlib_dict_del_string_key(_py, modules_ptr, resolved_key_bits);
            }

            let imported_bits = match importlib_import_with_fallback(
                _py,
                &resolved,
                resolved_key_bits,
                modules_ptr,
                util_bits,
                machinery_bits,
            ) {
                Ok(bits) => bits,
                Err(err) => {
                    if exception_pending(_py) {
                        importlib_rethrow_pending_exception(_py);
                    }
                    return err;
                }
            };

            let cached_bits =
                match importlib_dict_get_string_key_bits(_py, modules_ptr, resolved_key_bits) {
                    Ok(bits) => bits,
                    Err(err) => {
                        if !obj_from_bits(imported_bits).is_none() {
                            dec_ref_bits(_py, imported_bits);
                        }
                        return err;
                    }
                };
            if let Some(cached_bits) = cached_bits {
                if let Err(err) =
                    importlib_bind_submodule_on_parent(_py, &resolved, cached_bits, modules_ptr)
                {
                    return err;
                }
                inc_ref_bits(_py, cached_bits);
                if cached_bits != imported_bits && !obj_from_bits(imported_bits).is_none() {
                    dec_ref_bits(_py, imported_bits);
                }
                return cached_bits;
            }
            if !obj_from_bits(imported_bits).is_none() {
                if let Err(err) =
                    importlib_bind_submodule_on_parent(_py, &resolved, imported_bits, modules_ptr)
                {
                    if !obj_from_bits(imported_bits).is_none() {
                        dec_ref_bits(_py, imported_bits);
                    }
                    return err;
                }
                return imported_bits;
            }
            raise_exception::<_>(
                _py,
                "ModuleNotFoundError",
                &format!("No module named '{resolved}'"),
            )
        })();
        dec_ref_bits(_py, resolved_key_bits);
        if !obj_from_bits(modules_bits).is_none() {
            dec_ref_bits(_py, modules_bits);
        }
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_frozen_payload(machinery_bits: u64, util_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        static BUILTIN_IMPORTER_NAME: AtomicU64 = AtomicU64::new(0);
        static FROZEN_IMPORTER_NAME: AtomicU64 = AtomicU64::new(0);
        static MODULE_SPEC_NAME: AtomicU64 = AtomicU64::new(0);
        static MODULE_FROM_SPEC_NAME: AtomicU64 = AtomicU64::new(0);
        static SPEC_FROM_LOADER_NAME: AtomicU64 = AtomicU64::new(0);

        let mut owned: Vec<u64> = Vec::new();
        let mut values: Vec<(&[u8], u64)> = Vec::with_capacity(5);

        let builtin_importer_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &BUILTIN_IMPORTER_NAME,
            b"BuiltinImporter",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        owned.push(builtin_importer_bits);
        values.push((b"BuiltinImporter", builtin_importer_bits));

        let frozen_importer_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &FROZEN_IMPORTER_NAME,
            b"FrozenImporter",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(frozen_importer_bits);
        values.push((b"FrozenImporter", frozen_importer_bits));

        let module_spec_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &MODULE_SPEC_NAME,
            b"ModuleSpec",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(module_spec_bits);
        values.push((b"ModuleSpec", module_spec_bits));

        let module_from_spec_bits = match importlib_required_attribute(
            _py,
            util_bits,
            &MODULE_FROM_SPEC_NAME,
            b"module_from_spec",
            "importlib.util",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(module_from_spec_bits);
        values.push((b"module_from_spec", module_from_spec_bits));

        let spec_from_loader_bits = match importlib_required_attribute(
            _py,
            util_bits,
            &SPEC_FROM_LOADER_NAME,
            b"spec_from_loader",
            "importlib.util",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(spec_from_loader_bits);
        values.push((b"spec_from_loader", spec_from_loader_bits));

        let mut pairs: Vec<u64> = Vec::with_capacity(values.len() * 2);
        for (key, value_bits) in values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
        }

        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_typing_private_payload(typing_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        static GENERIC_NAME: AtomicU64 = AtomicU64::new(0);
        static PARAM_SPEC_NAME: AtomicU64 = AtomicU64::new(0);
        static PARAM_SPEC_ARGS_NAME: AtomicU64 = AtomicU64::new(0);
        static PARAM_SPEC_KWARGS_NAME: AtomicU64 = AtomicU64::new(0);
        static TYPE_ALIAS_TYPE_NAME: AtomicU64 = AtomicU64::new(0);
        static TYPE_VAR_NAME: AtomicU64 = AtomicU64::new(0);
        static TYPE_VAR_TUPLE_NAME: AtomicU64 = AtomicU64::new(0);

        let mut owned: Vec<u64> = Vec::new();
        let mut values: Vec<(&[u8], u64)> = Vec::with_capacity(7);

        let generic_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            &GENERIC_NAME,
            b"Generic",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(generic_bits);
        values.push((b"Generic", generic_bits));

        let param_spec_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            &PARAM_SPEC_NAME,
            b"_ParamSpec",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(param_spec_bits);
        values.push((b"ParamSpec", param_spec_bits));

        let param_spec_args_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            &PARAM_SPEC_ARGS_NAME,
            b"_ParamSpecArgs",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(param_spec_args_bits);
        values.push((b"ParamSpecArgs", param_spec_args_bits));

        let param_spec_kwargs_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            &PARAM_SPEC_KWARGS_NAME,
            b"_ParamSpecKwargs",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(param_spec_kwargs_bits);
        values.push((b"ParamSpecKwargs", param_spec_kwargs_bits));

        let type_alias_type_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            &TYPE_ALIAS_TYPE_NAME,
            b"_MoltTypeAlias",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(type_alias_type_bits);
        values.push((b"TypeAliasType", type_alias_type_bits));

        let type_var_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            &TYPE_VAR_NAME,
            b"_TypeVar",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(type_var_bits);
        values.push((b"TypeVar", type_var_bits));

        let type_var_tuple_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            &TYPE_VAR_TUPLE_NAME,
            b"_TypeVarTuple",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(type_var_tuple_bits);
        values.push((b"TypeVarTuple", type_var_tuple_bits));

        let mut pairs: Vec<u64> = Vec::with_capacity(values.len() * 2);
        for (key, value_bits) in values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
        }

        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_metadata_types_payload(
    typing_bits: u64,
    abc_bits: u64,
    contextlib_bits: u64,
    _itertools_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        static ANY_NAME: AtomicU64 = AtomicU64::new(0);
        static DICT_NAME: AtomicU64 = AtomicU64::new(0);
        static ITERATOR_NAME: AtomicU64 = AtomicU64::new(0);
        static LIST_NAME: AtomicU64 = AtomicU64::new(0);
        static META_PATH_FINDER_NAME: AtomicU64 = AtomicU64::new(0);
        static OPTIONAL_NAME: AtomicU64 = AtomicU64::new(0);
        static OVERLOAD_NAME: AtomicU64 = AtomicU64::new(0);
        static PROTOCOL_NAME: AtomicU64 = AtomicU64::new(0);
        static SUPPRESS_NAME: AtomicU64 = AtomicU64::new(0);
        static TYPE_VAR_NAME: AtomicU64 = AtomicU64::new(0);
        static UNION_NAME: AtomicU64 = AtomicU64::new(0);

        let mut owned: Vec<u64> = Vec::new();
        let mut values: Vec<(&[u8], u64)> = Vec::with_capacity(11);

        let any_bits =
            match importlib_required_attribute(_py, typing_bits, &ANY_NAME, b"Any", "typing") {
                Ok(bits) => bits,
                Err(err) => {
                    for bits in owned {
                        dec_ref_bits(_py, bits);
                    }
                    return err;
                }
            };
        owned.push(any_bits);
        values.push((b"Any", any_bits));

        let dict_bits =
            match importlib_required_attribute(_py, typing_bits, &DICT_NAME, b"Dict", "typing") {
                Ok(bits) => bits,
                Err(err) => {
                    for bits in owned {
                        dec_ref_bits(_py, bits);
                    }
                    return err;
                }
            };
        owned.push(dict_bits);
        values.push((b"Dict", dict_bits));

        let iterator_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            &ITERATOR_NAME,
            b"Iterator",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(iterator_bits);
        values.push((b"Iterator", iterator_bits));

        let list_bits =
            match importlib_required_attribute(_py, typing_bits, &LIST_NAME, b"List", "typing") {
                Ok(bits) => bits,
                Err(err) => {
                    for bits in owned {
                        dec_ref_bits(_py, bits);
                    }
                    return err;
                }
            };
        owned.push(list_bits);
        values.push((b"List", list_bits));

        let mapping_bits = dict_bits;
        inc_ref_bits(_py, mapping_bits);
        owned.push(mapping_bits);
        values.push((b"Mapping", mapping_bits));

        let optional_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            &OPTIONAL_NAME,
            b"Optional",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(optional_bits);
        values.push((b"Optional", optional_bits));

        let protocol_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            &PROTOCOL_NAME,
            b"Protocol",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(protocol_bits);
        values.push((b"Protocol", protocol_bits));

        let type_var_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            &TYPE_VAR_NAME,
            b"_TypeVar",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(type_var_bits);
        values.push((b"TypeVar", type_var_bits));

        let union_bits =
            match importlib_required_attribute(_py, typing_bits, &UNION_NAME, b"Union", "typing") {
                Ok(bits) => bits,
                Err(err) => {
                    for bits in owned {
                        dec_ref_bits(_py, bits);
                    }
                    return err;
                }
            };
        owned.push(union_bits);
        values.push((b"Union", union_bits));

        let overload_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            &OVERLOAD_NAME,
            b"overload",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(overload_bits);
        values.push((b"overload", overload_bits));

        let meta_path_finder_bits = match importlib_required_attribute(
            _py,
            abc_bits,
            &META_PATH_FINDER_NAME,
            b"MetaPathFinder",
            "importlib.abc",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(meta_path_finder_bits);
        values.push((b"MetaPathFinder", meta_path_finder_bits));

        let suppress_bits = match importlib_required_attribute(
            _py,
            contextlib_bits,
            &SUPPRESS_NAME,
            b"suppress",
            "contextlib",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(suppress_bits);
        values.push((b"suppress", suppress_bits));

        let mut pairs: Vec<u64> = Vec::with_capacity(values.len() * 2);
        for (key, value_bits) in values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
        }

        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_frozen_external_payload(
    machinery_bits: u64,
    util_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        static BYTECODE_SUFFIXES_NAME: AtomicU64 = AtomicU64::new(0);
        static DEBUG_BYTECODE_SUFFIXES_NAME: AtomicU64 = AtomicU64::new(0);
        static EXTENSION_SUFFIXES_NAME: AtomicU64 = AtomicU64::new(0);
        static MAGIC_NUMBER_NAME: AtomicU64 = AtomicU64::new(0);
        static OPTIMIZED_BYTECODE_SUFFIXES_NAME: AtomicU64 = AtomicU64::new(0);
        static SOURCE_SUFFIXES_NAME: AtomicU64 = AtomicU64::new(0);
        static EXTENSION_FILE_LOADER_NAME: AtomicU64 = AtomicU64::new(0);
        static FILE_FINDER_NAME: AtomicU64 = AtomicU64::new(0);
        static PRIVATE_FILE_LOADER_NAME: AtomicU64 = AtomicU64::new(0);
        static NAMESPACE_LOADER_NAME: AtomicU64 = AtomicU64::new(0);
        static PATH_FINDER_NAME: AtomicU64 = AtomicU64::new(0);
        static SOURCE_FILE_LOADER_NAME: AtomicU64 = AtomicU64::new(0);
        static PRIVATE_SOURCE_LOADER_NAME: AtomicU64 = AtomicU64::new(0);
        static SOURCELESS_FILE_LOADER_NAME: AtomicU64 = AtomicU64::new(0);
        static LOADER_BASICS_NAME: AtomicU64 = AtomicU64::new(0);
        static WINDOWS_REGISTRY_FINDER_NAME: AtomicU64 = AtomicU64::new(0);
        static CACHE_FROM_SOURCE_NAME: AtomicU64 = AtomicU64::new(0);
        static DECODE_SOURCE_NAME: AtomicU64 = AtomicU64::new(0);
        static SOURCE_FROM_CACHE_NAME: AtomicU64 = AtomicU64::new(0);
        static SPEC_FROM_FILE_LOCATION_NAME: AtomicU64 = AtomicU64::new(0);

        let mut owned: Vec<u64> = Vec::new();
        let mut values: Vec<(&[u8], u64)> = Vec::with_capacity(20);

        let bytecode_suffixes_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &BYTECODE_SUFFIXES_NAME,
            b"BYTECODE_SUFFIXES",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(bytecode_suffixes_bits);
        values.push((b"BYTECODE_SUFFIXES", bytecode_suffixes_bits));

        let debug_bytecode_suffixes_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &DEBUG_BYTECODE_SUFFIXES_NAME,
            b"DEBUG_BYTECODE_SUFFIXES",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(debug_bytecode_suffixes_bits);
        values.push((b"DEBUG_BYTECODE_SUFFIXES", debug_bytecode_suffixes_bits));

        let extension_suffixes_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &EXTENSION_SUFFIXES_NAME,
            b"EXTENSION_SUFFIXES",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(extension_suffixes_bits);
        values.push((b"EXTENSION_SUFFIXES", extension_suffixes_bits));

        let magic_number_bits = match importlib_required_attribute(
            _py,
            util_bits,
            &MAGIC_NUMBER_NAME,
            b"MAGIC_NUMBER",
            "importlib.util",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(magic_number_bits);
        values.push((b"MAGIC_NUMBER", magic_number_bits));

        let optimized_bytecode_suffixes_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &OPTIMIZED_BYTECODE_SUFFIXES_NAME,
            b"OPTIMIZED_BYTECODE_SUFFIXES",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(optimized_bytecode_suffixes_bits);
        values.push((
            b"OPTIMIZED_BYTECODE_SUFFIXES",
            optimized_bytecode_suffixes_bits,
        ));

        let source_suffixes_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &SOURCE_SUFFIXES_NAME,
            b"SOURCE_SUFFIXES",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(source_suffixes_bits);
        values.push((b"SOURCE_SUFFIXES", source_suffixes_bits));

        let extension_file_loader_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &EXTENSION_FILE_LOADER_NAME,
            b"ExtensionFileLoader",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(extension_file_loader_bits);
        values.push((b"ExtensionFileLoader", extension_file_loader_bits));

        let file_finder_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &FILE_FINDER_NAME,
            b"FileFinder",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(file_finder_bits);
        values.push((b"FileFinder", file_finder_bits));

        let file_loader_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &PRIVATE_FILE_LOADER_NAME,
            b"_FileLoader",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(file_loader_bits);
        values.push((b"FileLoader", file_loader_bits));

        let namespace_loader_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &NAMESPACE_LOADER_NAME,
            b"NamespaceLoader",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(namespace_loader_bits);
        values.push((b"NamespaceLoader", namespace_loader_bits));

        let path_finder_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &PATH_FINDER_NAME,
            b"PathFinder",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(path_finder_bits);
        values.push((b"PathFinder", path_finder_bits));

        let source_file_loader_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &SOURCE_FILE_LOADER_NAME,
            b"SourceFileLoader",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(source_file_loader_bits);
        values.push((b"SourceFileLoader", source_file_loader_bits));

        let source_loader_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &PRIVATE_SOURCE_LOADER_NAME,
            b"_SourceLoader",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(source_loader_bits);
        values.push((b"SourceLoader", source_loader_bits));

        let sourceless_file_loader_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &SOURCELESS_FILE_LOADER_NAME,
            b"SourcelessFileLoader",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(sourceless_file_loader_bits);
        values.push((b"SourcelessFileLoader", sourceless_file_loader_bits));

        let loader_basics_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &LOADER_BASICS_NAME,
            b"_LoaderBasics",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(loader_basics_bits);
        values.push((b"_LoaderBasics", loader_basics_bits));

        let windows_registry_finder_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &WINDOWS_REGISTRY_FINDER_NAME,
            b"WindowsRegistryFinder",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(windows_registry_finder_bits);
        values.push((b"WindowsRegistryFinder", windows_registry_finder_bits));

        let cache_from_source_bits = match importlib_required_attribute(
            _py,
            util_bits,
            &CACHE_FROM_SOURCE_NAME,
            b"cache_from_source",
            "importlib.util",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(cache_from_source_bits);
        values.push((b"cache_from_source", cache_from_source_bits));

        let decode_source_bits = match importlib_required_attribute(
            _py,
            util_bits,
            &DECODE_SOURCE_NAME,
            b"decode_source",
            "importlib.util",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(decode_source_bits);
        values.push((b"decode_source", decode_source_bits));

        let source_from_cache_bits = match importlib_required_attribute(
            _py,
            util_bits,
            &SOURCE_FROM_CACHE_NAME,
            b"source_from_cache",
            "importlib.util",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(source_from_cache_bits);
        values.push((b"source_from_cache", source_from_cache_bits));

        let spec_from_file_location_bits = match importlib_required_attribute(
            _py,
            util_bits,
            &SPEC_FROM_FILE_LOCATION_NAME,
            b"spec_from_file_location",
            "importlib.util",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(spec_from_file_location_bits);
        values.push((b"spec_from_file_location", spec_from_file_location_bits));

        let mut pairs: Vec<u64> = Vec::with_capacity(values.len() * 2);
        for (key, value_bits) in values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_find_in_path(fullname_bits: u64, search_paths_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        importlib_find_in_path_payload(_py, fullname_bits, search_paths_bits, false)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_find_in_path_package_context(
    fullname_bits: u64,
    search_paths_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
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

struct ImportlibPathHooksContext<'a> {
    fullname: &'a str,
    search_paths: &'a [String],
    module_file: Option<String>,
    path_hooks_bits: u64,
    path_importer_cache_bits: u64,
    package_context: bool,
    machinery_bits: u64,
}

fn importlib_find_spec_from_path_hooks_impl(
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
    if ctx.fullname != "math" && !has_capability(_py, "fs.read") {
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

struct ImportlibRuntimeSpecContext<'a> {
    fullname: &'a str,
    search_paths: &'a [String],
    module_file: Option<String>,
    meta_path_bits: u64,
    path_hooks_bits: u64,
    path_importer_cache_bits: u64,
    package_context: bool,
    machinery_bits: u64,
}

fn importlib_find_spec_with_runtime_state_bits(
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
    if ctx.fullname != "math" && !has_capability(_py, "fs.read") {
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

fn importlib_find_spec_orchestrated_search_paths(
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

    static SUBMODULE_SEARCH_LOCATIONS_NAME: AtomicU64 = AtomicU64::new(0);
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
        &SUBMODULE_SEARCH_LOCATIONS_NAME,
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

fn importlib_find_spec_orchestrated_impl(
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

fn importlib_machinery_module_file(
    _py: &PyToken<'_>,
    machinery_bits: u64,
) -> Result<Option<String>, u64> {
    static FILE_NAME: AtomicU64 = AtomicU64::new(0);
    let file_name = intern_static_name(_py, &FILE_NAME, b"__file__");
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

fn importlib_runtime_path_hooks_and_cache_bits(
    _py: &PyToken<'_>,
    sys_bits: Option<u64>,
) -> Result<(u64, bool, u64, bool), u64> {
    static PATH_HOOKS_NAME: AtomicU64 = AtomicU64::new(0);
    static PATH_IMPORTER_CACHE_NAME: AtomicU64 = AtomicU64::new(0);

    let mut path_hooks_bits = MoltObject::none().bits();
    let mut owns_path_hooks = false;
    let mut path_importer_cache_bits = MoltObject::none().bits();
    let mut owns_path_importer_cache = false;

    if let Some(sys_bits) = sys_bits
        && !obj_from_bits(sys_bits).is_none()
    {
        let path_hooks_name = intern_static_name(_py, &PATH_HOOKS_NAME, b"path_hooks");
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

        let path_importer_cache_name =
            intern_static_name(_py, &PATH_IMPORTER_CACHE_NAME, b"path_importer_cache");
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
    crate::with_gil_entry!(_py, {
        static PATH_NAME: AtomicU64 = AtomicU64::new(0);

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
                let path_name = intern_static_name(_py, &PATH_NAME, b"path");
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        static SPEC_CACHE_NAME: AtomicU64 = AtomicU64::new(0);
        static PATH_IMPORTER_CACHE_NAME: AtomicU64 = AtomicU64::new(0);

        if let Some(util_bits) = importlib_module_cache_lookup_bits(_py, "importlib.util")
            && !obj_from_bits(util_bits).is_none()
        {
            importlib_clear_mapping_attr_best_effort(
                _py,
                util_bits,
                &SPEC_CACHE_NAME,
                b"_SPEC_CACHE",
            );
        }
        if let Some(sys_bits) = importlib_module_cache_lookup_bits(_py, "sys")
            && !obj_from_bits(sys_bits).is_none()
        {
            importlib_clear_mapping_attr_best_effort(
                _py,
                sys_bits,
                &PATH_IMPORTER_CACHE_NAME,
                b"path_importer_cache",
            );
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_filefinder_invalidate(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        static PATH_IMPORTER_CACHE_NAME: AtomicU64 = AtomicU64::new(0);
        static POP_NAME: AtomicU64 = AtomicU64::new(0);

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
        let path_importer_cache_name =
            intern_static_name(_py, &PATH_IMPORTER_CACHE_NAME, b"path_importer_cache");
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
                    &POP_NAME,
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_reload(
    module_bits: u64,
    util_bits: u64,
    machinery_bits: u64,
    import_module_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        static NAME_NAME: AtomicU64 = AtomicU64::new(0);
        static FILE_NAME: AtomicU64 = AtomicU64::new(0);
        static SPEC_NAME: AtomicU64 = AtomicU64::new(0);
        static LOADER_NAME: AtomicU64 = AtomicU64::new(0);
        static PATH_NAME: AtomicU64 = AtomicU64::new(0);
        static SPEC_FROM_FILE_LOCATION_NAME: AtomicU64 = AtomicU64::new(0);
        static FIND_SPEC_NAME: AtomicU64 = AtomicU64::new(0);
        static EXEC_MODULE_NAME: AtomicU64 = AtomicU64::new(0);
        static LOAD_MODULE_NAME: AtomicU64 = AtomicU64::new(0);

        let mut name_bits = MoltObject::none().bits();
        let mut module_loader_bits = MoltObject::none().bits();
        let mut module_loader_owned = false;
        let mut modules_bits = MoltObject::none().bits();

        let out = (|| -> Result<u64, u64> {
            let module_name_name = intern_static_name(_py, &NAME_NAME, b"__name__");
            let spec_name = intern_static_name(_py, &SPEC_NAME, b"__spec__");
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

            let module_file_name = intern_static_name(_py, &FILE_NAME, b"__file__");
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

            let loader_name = intern_static_name(_py, &LOADER_NAME, b"loader");
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
                let path_name = intern_static_name(_py, &PATH_NAME, b"__path__");
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
                    &SPEC_FROM_FILE_LOCATION_NAME,
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
                            &EXEC_MODULE_NAME,
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
                    &EXEC_MODULE_NAME,
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
                &FIND_SPEC_NAME,
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
                        &EXEC_MODULE_NAME,
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
                        &LOAD_MODULE_NAME,
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
    crate::with_gil_entry!(_py, {
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
        if fullname != "math" && !has_capability(_py, "fs.read") {
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        match importlib_runtime_state_payload_bits(_py) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_runtime_modules() -> u64 {
    crate::with_gil_entry!(_py, {
        match importlib_runtime_modules_bits(_py) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_runtime_state_view() -> u64 {
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        static SPEC_NAME: AtomicU64 = AtomicU64::new(0);
        static FILE_NAME: AtomicU64 = AtomicU64::new(0);
        static MODULE_SPEC_NAME: AtomicU64 = AtomicU64::new(0);

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

        let spec_name = intern_static_name(_py, &SPEC_NAME, b"__spec__");
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

        let file_name = intern_static_name(_py, &FILE_NAME, b"__file__");
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
            &MODULE_SPEC_NAME,
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
    crate::with_gil_entry!(_py, {
        static DUNDER_PATH_NAME: AtomicU64 = AtomicU64::new(0);

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
                    let path_name = intern_static_name(_py, &DUNDER_PATH_NAME, b"__path__");
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
    crate::with_gil_entry!(_py, {
        static DEFAULT_META_PATH_BOOTSTRAPPED: AtomicU64 = AtomicU64::new(0);
        static META_PATH_NAME: AtomicU64 = AtomicU64::new(0);
        static PATH_FINDER_NAME: AtomicU64 = AtomicU64::new(0);

        let mark_bootstrapped = || {
            DEFAULT_META_PATH_BOOTSTRAPPED.store(1, Ordering::Relaxed);
            MoltObject::none().bits()
        };

        if DEFAULT_META_PATH_BOOTSTRAPPED.load(Ordering::Relaxed) != 0 {
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

        let meta_path_name = intern_static_name(_py, &META_PATH_NAME, b"meta_path");
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

        let path_finder_name = intern_static_name(_py, &PATH_FINDER_NAME, b"PathFinder");
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
    crate::with_gil_entry!(_py, {
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
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_path_payload(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let payload = importlib_resources_path_payload(&path);
        let basename_bits = match alloc_str_bits(_py, &payload.basename) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let entries_bits = match alloc_string_list_bits(_py, &payload.entries) {
            Some(bits) => bits,
            None => {
                dec_ref_bits(_py, basename_bits);
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
        };
        let exists_bits = MoltObject::from_bool(payload.exists).bits();
        let is_file_bits = MoltObject::from_bool(payload.is_file).bits();
        let is_dir_bits = MoltObject::from_bool(payload.is_dir).bits();
        let has_init_py_bits = MoltObject::from_bool(payload.has_init_py).bits();
        let is_archive_member_bits = MoltObject::from_bool(payload.is_archive_member).bits();
        let keys_and_values: [(&[u8], u64); 7] = [
            (b"basename", basename_bits),
            (b"exists", exists_bits),
            (b"is_file", is_file_bits),
            (b"is_dir", is_dir_bits),
            (b"entries", entries_bits),
            (b"has_init_py", has_init_py_bits),
            (b"is_archive_member", is_archive_member_bits),
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
pub extern "C" fn molt_importlib_resources_package_info(
    package_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
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
        let payload = importlib_resources_package_payload(&package, &search_paths, module_file);
        let roots_bits = match alloc_string_list_bits(_py, &payload.roots) {
            Some(bits) => bits,
            None => return raise_exception::<_>(_py, "MemoryError", "out of memory"),
        };
        let init_file_bits = match payload.init_file.as_deref() {
            Some(path) => match alloc_str_bits(_py, path) {
                Ok(bits) => bits,
                Err(err) => {
                    dec_ref_bits(_py, roots_bits);
                    return err;
                }
            },
            None => MoltObject::none().bits(),
        };
        let is_namespace_bits = MoltObject::from_bool(payload.is_namespace).bits();
        let has_regular_package_bits = MoltObject::from_bool(payload.has_regular_package).bits();
        let tuple_ptr = alloc_tuple(
            _py,
            &[
                roots_bits,
                is_namespace_bits,
                has_regular_package_bits,
                init_file_bits,
            ],
        );
        dec_ref_bits(_py, roots_bits);
        if !obj_from_bits(init_file_bits).is_none() {
            dec_ref_bits(_py, init_file_bits);
        }
        if tuple_ptr.is_null() {
            raise_exception::<_>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_open_resource_bytes_from_package(
    package_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
    resource_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
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
        let resource = match string_arg_from_bits(_py, resource_bits, "resource name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let bytes = match importlib_resources_open_resource_bytes_from_package_impl(
            _py,
            &package,
            &search_paths,
            module_file,
            &resource,
        ) {
            Ok(value) => value,
            Err(err) => return err,
        };
        let out_ptr = alloc_bytes(_py, &bytes);
        if out_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_open_resource_bytes_from_package_parts(
    package_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
    path_parts_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
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
        let path_parts = match string_sequence_arg_from_bits(_py, path_parts_bits, "path names") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let bytes = match importlib_resources_open_resource_bytes_from_package_parts_impl(
            _py,
            &package,
            &search_paths,
            module_file,
            &path_parts,
        ) {
            Ok(value) => value,
            Err(err) => return err,
        };
        let out_ptr = alloc_bytes(_py, &bytes);
        if out_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_read_text_from_package(
    package_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
    resource_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        static DECODE_NAME: AtomicU64 = AtomicU64::new(0);
        if !has_capability(_py, "fs.read") {
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
        let resource = match string_arg_from_bits(_py, resource_bits, "resource name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let encoding = match string_arg_from_bits(_py, encoding_bits, "encoding") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let errors = match string_arg_from_bits(_py, errors_bits, "errors") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let bytes = match importlib_resources_open_resource_bytes_from_package_impl(
            _py,
            &package,
            &search_paths,
            module_file,
            &resource,
        ) {
            Ok(value) => value,
            Err(err) => return err,
        };
        let bytes_ptr = alloc_bytes(_py, &bytes);
        if bytes_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        let bytes_bits = MoltObject::from_ptr(bytes_ptr).bits();
        let decode_name = intern_static_name(_py, &DECODE_NAME, b"decode");
        let decode_bits = match importlib_reader_lookup_callable(_py, bytes_bits, decode_name) {
            Ok(Some(bits)) => bits,
            Ok(None) => {
                dec_ref_bits(_py, bytes_bits);
                return raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "invalid bytes decode callable payload",
                );
            }
            Err(err) => {
                dec_ref_bits(_py, bytes_bits);
                return err;
            }
        };
        let encoding_arg_bits = match alloc_str_bits(_py, &encoding) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, decode_bits);
                dec_ref_bits(_py, bytes_bits);
                return err;
            }
        };
        let errors_arg_bits = match alloc_str_bits(_py, &errors) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, encoding_arg_bits);
                dec_ref_bits(_py, decode_bits);
                dec_ref_bits(_py, bytes_bits);
                return err;
            }
        };
        let out_bits =
            match call_callable_positional(_py, decode_bits, &[encoding_arg_bits, errors_arg_bits])
            {
                Ok(bits) => bits,
                Err(err) => {
                    dec_ref_bits(_py, errors_arg_bits);
                    dec_ref_bits(_py, encoding_arg_bits);
                    dec_ref_bits(_py, decode_bits);
                    dec_ref_bits(_py, bytes_bits);
                    return err;
                }
            };
        dec_ref_bits(_py, errors_arg_bits);
        dec_ref_bits(_py, encoding_arg_bits);
        dec_ref_bits(_py, decode_bits);
        dec_ref_bits(_py, bytes_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        out_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_read_text_from_package_parts(
    package_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
    path_parts_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        static DECODE_NAME: AtomicU64 = AtomicU64::new(0);
        if !has_capability(_py, "fs.read") {
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
        let path_parts = match string_sequence_arg_from_bits(_py, path_parts_bits, "path names") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let encoding = match string_arg_from_bits(_py, encoding_bits, "encoding") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let errors = match string_arg_from_bits(_py, errors_bits, "errors") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let bytes = match importlib_resources_open_resource_bytes_from_package_parts_impl(
            _py,
            &package,
            &search_paths,
            module_file,
            &path_parts,
        ) {
            Ok(value) => value,
            Err(err) => return err,
        };
        let bytes_ptr = alloc_bytes(_py, &bytes);
        if bytes_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        let bytes_bits = MoltObject::from_ptr(bytes_ptr).bits();
        let decode_name = intern_static_name(_py, &DECODE_NAME, b"decode");
        let decode_bits = match importlib_reader_lookup_callable(_py, bytes_bits, decode_name) {
            Ok(Some(bits)) => bits,
            Ok(None) => {
                dec_ref_bits(_py, bytes_bits);
                return raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "invalid bytes decode callable payload",
                );
            }
            Err(err) => {
                dec_ref_bits(_py, bytes_bits);
                return err;
            }
        };
        let encoding_arg_bits = match alloc_str_bits(_py, &encoding) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, decode_bits);
                dec_ref_bits(_py, bytes_bits);
                return err;
            }
        };
        let errors_arg_bits = match alloc_str_bits(_py, &errors) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, encoding_arg_bits);
                dec_ref_bits(_py, decode_bits);
                dec_ref_bits(_py, bytes_bits);
                return err;
            }
        };
        let out_bits =
            match call_callable_positional(_py, decode_bits, &[encoding_arg_bits, errors_arg_bits])
            {
                Ok(bits) => bits,
                Err(err) => {
                    dec_ref_bits(_py, errors_arg_bits);
                    dec_ref_bits(_py, encoding_arg_bits);
                    dec_ref_bits(_py, decode_bits);
                    dec_ref_bits(_py, bytes_bits);
                    return err;
                }
            };
        dec_ref_bits(_py, errors_arg_bits);
        dec_ref_bits(_py, encoding_arg_bits);
        dec_ref_bits(_py, decode_bits);
        dec_ref_bits(_py, bytes_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        out_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_contents_from_package(
    package_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
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
        let payload = match importlib_resources_required_package_payload(
            _py,
            &package,
            &search_paths,
            module_file,
        ) {
            Ok(value) => value,
            Err(err) => return err,
        };
        let mut entries: BTreeSet<String> = BTreeSet::new();
        let mut has_init_py = false;
        for root in &payload.roots {
            let root_payload = importlib_resources_path_payload(root);
            has_init_py = has_init_py || root_payload.has_init_py;
            for entry in root_payload.entries {
                entries.insert(entry);
            }
        }
        if has_init_py {
            entries.insert(String::from("__pycache__"));
        }
        let out: Vec<String> = entries.into_iter().collect();
        match alloc_string_list_bits(_py, &out) {
            Some(bits) => bits,
            None => raise_exception::<_>(_py, "MemoryError", "out of memory"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_contents_from_package_parts(
    package_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
    path_parts_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
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
        let path_parts = match string_sequence_arg_from_bits(_py, path_parts_bits, "path names") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let out = match importlib_resources_contents_from_package_parts_impl(
            _py,
            &package,
            &search_paths,
            module_file,
            &path_parts,
        ) {
            Ok(value) => value,
            Err(err) => return err,
        };
        match alloc_string_list_bits(_py, &out) {
            Some(bits) => bits,
            None => raise_exception::<_>(_py, "MemoryError", "out of memory"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_is_resource_from_package(
    package_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
    resource_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
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
        let resource = match string_arg_from_bits(_py, resource_bits, "resource name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if let Err(err) = importlib_validate_resource_name_text(_py, &resource) {
            return err;
        }
        let payload = match importlib_resources_required_package_payload(
            _py,
            &package,
            &search_paths,
            module_file,
        ) {
            Ok(value) => value,
            Err(err) => return err,
        };
        MoltObject::from_bool(
            importlib_resources_first_file_candidate(&payload.roots, &resource).is_some(),
        )
        .bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_is_resource_from_package_parts(
    package_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
    path_parts_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
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
        let path_parts = match string_sequence_arg_from_bits(_py, path_parts_bits, "path names") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match importlib_resources_is_resource_from_package_parts_impl(
            _py,
            &package,
            &search_paths,
            module_file,
            &path_parts,
        ) {
            Ok(value) => MoltObject::from_bool(value).bits(),
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_resource_path_from_package(
    package_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
    resource_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
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
        let resource = match string_arg_from_bits(_py, resource_bits, "resource name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if let Err(err) = importlib_validate_resource_name_text(_py, &resource) {
            return err;
        }
        let payload = match importlib_resources_required_package_payload(
            _py,
            &package,
            &search_paths,
            module_file,
        ) {
            Ok(value) => value,
            Err(err) => return err,
        };
        if let Some(candidate) =
            importlib_resources_first_fs_file_candidate(&payload.roots, &resource)
        {
            return match alloc_str_bits(_py, &candidate) {
                Ok(bits) => bits,
                Err(err) => err,
            };
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_resource_path_from_package_parts(
    package_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
    path_parts_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
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
        let path_parts = match string_sequence_arg_from_bits(_py, path_parts_bits, "path names") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match importlib_resources_resource_path_from_package_parts_impl(
            _py,
            &package,
            &search_paths,
            module_file,
            &path_parts,
        ) {
            Ok(Some(path)) => match alloc_str_bits(_py, &path) {
                Ok(bits) => bits,
                Err(err) => err,
            },
            Ok(None) => MoltObject::none().bits(),
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_as_file_enter(
    traversable_bits: u64,
    traversable_type_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if isinstance_bits(_py, traversable_bits, traversable_type_bits) {
            inc_ref_bits(_py, traversable_bits);
            return traversable_bits;
        }
        let path = match path_from_bits(_py, traversable_bits) {
            Ok(value) => value,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let path_text = path.to_string_lossy().into_owned();
        let path_bits = match alloc_str_bits(_py, &path_text) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let result_bits = unsafe { call_callable1(_py, traversable_type_bits, path_bits) };
        dec_ref_bits(_py, path_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        result_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_as_file_exit(
    _exc_type_bits: u64,
    _exc_bits: u64,
    _tb_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(false).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_module_name(
    module_bits: u64,
    fallback_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let name = match importlib_resources_module_name_from_bits(_py, module_bits, fallback_bits)
        {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match alloc_str_bits(_py, &name) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_loader_reader(
    module_bits: u64,
    module_name_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module_name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match importlib_resources_loader_reader_from_bits(_py, module_bits, &module_name) {
            Ok(Some(bits)) => bits,
            Ok(None) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_files_payload(
    module_bits: u64,
    fallback_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let search_paths =
            match string_sequence_arg_from_bits(_py, search_paths_bits, "search paths") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let payload = match importlib_resources_files_payload(
            _py,
            module_bits,
            fallback_bits,
            &search_paths,
            module_file,
        ) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let package_name_bits = match alloc_str_bits(_py, &payload.package_name) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let roots_bits = match alloc_string_list_bits(_py, &payload.roots) {
            Some(bits) => bits,
            None => {
                dec_ref_bits(_py, package_name_bits);
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
        };
        let is_namespace_bits = MoltObject::from_bool(payload.is_namespace).bits();
        let reader_bits = payload.reader_bits.unwrap_or(MoltObject::none().bits());
        let files_traversable_bits = payload
            .files_traversable_bits
            .unwrap_or(MoltObject::none().bits());
        let keys_and_values: [(&[u8], u64); 5] = [
            (b"package_name", package_name_bits),
            (b"roots", roots_bits),
            (b"is_namespace", is_namespace_bits),
            (b"reader", reader_bits),
            (b"files_traversable", files_traversable_bits),
        ];
        let mut pairs: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        let mut owned: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        for (key, value_bits) in keys_and_values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
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
            raise_exception::<_>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(dict_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_reader_files_traversable(reader_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        match importlib_reader_files_traversable_bits(_py, reader_bits) {
            Ok(Some(bits)) => bits,
            Ok(None) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_reader_roots(reader_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let roots = match importlib_resources_reader_roots_impl(_py, reader_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match alloc_string_list_bits(_py, &roots) {
            Some(bits) => bits,
            None => raise_exception::<_>(_py, "MemoryError", "out of memory"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_reader_contents(reader_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let entries = match importlib_resources_reader_contents_impl(_py, reader_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match alloc_string_list_bits(_py, &entries) {
            Some(bits) => bits,
            None => raise_exception::<_>(_py, "MemoryError", "out of memory"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_reader_resource_path(
    reader_bits: u64,
    name_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let name = match string_arg_from_bits(_py, name_bits, "name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let path = match importlib_resources_reader_resource_path_impl(_py, reader_bits, &name) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match path {
            Some(value) => match alloc_str_bits(_py, &value) {
                Ok(bits) => bits,
                Err(err) => err,
            },
            None => MoltObject::none().bits(),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_reader_is_resource(
    reader_bits: u64,
    name_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let name = match string_arg_from_bits(_py, name_bits, "name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match importlib_resources_reader_is_resource_impl(_py, reader_bits, &name) {
            Ok(value) => MoltObject::from_bool(value).bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_reader_open_resource_bytes(
    reader_bits: u64,
    name_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let name = match string_arg_from_bits(_py, name_bits, "name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let payload =
            match importlib_resources_reader_open_resource_bytes_impl(_py, reader_bits, &name) {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let out_ptr = alloc_bytes(_py, &payload);
        if out_ptr.is_null() {
            raise_exception::<_>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_reader_child_names(
    reader_bits: u64,
    parts_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let parts = match string_sequence_arg_from_bits(_py, parts_bits, "parts") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let entries = match importlib_resources_reader_child_names_impl(_py, reader_bits, &parts) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match alloc_string_list_bits(_py, &entries) {
            Some(bits) => bits,
            None => raise_exception::<_>(_py, "MemoryError", "out of memory"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_reader_exists(reader_bits: u64, parts_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let parts = match string_sequence_arg_from_bits(_py, parts_bits, "parts") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match importlib_resources_reader_exists_impl(_py, reader_bits, &parts) {
            Ok(value) => MoltObject::from_bool(value).bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_reader_is_dir(reader_bits: u64, parts_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let parts = match string_sequence_arg_from_bits(_py, parts_bits, "parts") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match importlib_resources_reader_is_dir_impl(_py, reader_bits, &parts) {
            Ok(value) => MoltObject::from_bool(value).bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_metadata_dist_paths(
    search_paths_bits: u64,
    module_file_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let search_paths =
            match string_sequence_arg_from_bits(_py, search_paths_bits, "search paths") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let resolved = importlib_metadata_dist_paths(&search_paths, module_file);
        match alloc_string_list_bits(_py, &resolved) {
            Some(bits) => bits,
            None => raise_exception::<_>(_py, "MemoryError", "out of memory"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_metadata_entry_points_payload(
    search_paths_bits: u64,
    module_file_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let search_paths =
            match string_sequence_arg_from_bits(_py, search_paths_bits, "search paths") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let payload = importlib_metadata_entry_points_payload(&search_paths, module_file);
        match alloc_string_triplets_list_bits(_py, &payload) {
            Some(bits) => bits,
            None => raise_exception::<_>(_py, "MemoryError", "out of memory"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_metadata_entry_points_select_payload(
    search_paths_bits: u64,
    module_file_bits: u64,
    group_bits: u64,
    name_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let search_paths =
            match string_sequence_arg_from_bits(_py, search_paths_bits, "search paths") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let group = match optional_string_arg_from_bits(_py, group_bits, "group") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let name = match optional_string_arg_from_bits(_py, name_bits, "name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let payload = importlib_metadata_entry_points_select_payload(
            &search_paths,
            module_file,
            group.as_deref(),
            name.as_deref(),
        );
        match alloc_string_triplets_list_bits(_py, &payload) {
            Some(bits) => bits,
            None => raise_exception::<_>(_py, "MemoryError", "out of memory"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_metadata_entry_points_filter_payload(
    search_paths_bits: u64,
    module_file_bits: u64,
    group_bits: u64,
    name_bits: u64,
    value_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let search_paths =
            match string_sequence_arg_from_bits(_py, search_paths_bits, "search paths") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let group = match optional_string_arg_from_bits(_py, group_bits, "group") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let name = match optional_string_arg_from_bits(_py, name_bits, "name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let value = match optional_string_arg_from_bits(_py, value_bits, "value") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let payload = importlib_metadata_entry_points_filter_payload(
            &search_paths,
            module_file,
            group.as_deref(),
            name.as_deref(),
            value.as_deref(),
        );
        match alloc_string_triplets_list_bits(_py, &payload) {
            Some(bits) => bits,
            None => raise_exception::<_>(_py, "MemoryError", "out of memory"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_metadata_normalize_name(name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name = match string_arg_from_bits(_py, name_bits, "name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match alloc_str_bits(_py, &importlib_metadata_normalize_name(&name)) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

fn alloc_importlib_metadata_payload_dict_bits(
    _py: &PyToken<'_>,
    payload: &ImportlibMetadataPayload,
) -> Result<u64, u64> {
    let path_bits = alloc_str_bits(_py, &payload.path)?;
    let name_bits = match alloc_str_bits(_py, &payload.name) {
        Ok(bits) => bits,
        Err(err) => {
            dec_ref_bits(_py, path_bits);
            return Err(err);
        }
    };
    let version_bits = match alloc_str_bits(_py, &payload.version) {
        Ok(bits) => bits,
        Err(err) => {
            dec_ref_bits(_py, path_bits);
            dec_ref_bits(_py, name_bits);
            return Err(err);
        }
    };
    let metadata_bits = match alloc_string_pairs_dict_bits(_py, &payload.metadata) {
        Some(bits) => bits,
        None => {
            dec_ref_bits(_py, path_bits);
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, version_bits);
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
    };
    let entry_points_bits = match alloc_string_triplets_list_bits(_py, &payload.entry_points) {
        Some(bits) => bits,
        None => {
            dec_ref_bits(_py, path_bits);
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, version_bits);
            dec_ref_bits(_py, metadata_bits);
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
    };
    let requires_dist_bits = match alloc_string_list_bits(_py, &payload.requires_dist) {
        Some(bits) => bits,
        None => {
            dec_ref_bits(_py, path_bits);
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, version_bits);
            dec_ref_bits(_py, metadata_bits);
            dec_ref_bits(_py, entry_points_bits);
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
    };
    let provides_extra_bits = match alloc_string_list_bits(_py, &payload.provides_extra) {
        Some(bits) => bits,
        None => {
            dec_ref_bits(_py, path_bits);
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, version_bits);
            dec_ref_bits(_py, metadata_bits);
            dec_ref_bits(_py, entry_points_bits);
            dec_ref_bits(_py, requires_dist_bits);
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
    };
    let requires_python_bits = match payload.requires_python.as_deref() {
        Some(value) => match alloc_str_bits(_py, value) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, path_bits);
                dec_ref_bits(_py, name_bits);
                dec_ref_bits(_py, version_bits);
                dec_ref_bits(_py, metadata_bits);
                dec_ref_bits(_py, entry_points_bits);
                dec_ref_bits(_py, requires_dist_bits);
                dec_ref_bits(_py, provides_extra_bits);
                return Err(err);
            }
        },
        None => MoltObject::none().bits(),
    };
    let keys_and_values: [(&[u8], u64); 8] = [
        (b"path", path_bits),
        (b"name", name_bits),
        (b"version", version_bits),
        (b"metadata", metadata_bits),
        (b"entry_points", entry_points_bits),
        (b"requires_dist", requires_dist_bits),
        (b"provides_extra", provides_extra_bits),
        (b"requires_python", requires_python_bits),
    ];
    let mut pairs: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
    let mut owned: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
    for (key, value_bits) in keys_and_values {
        let key_ptr = alloc_string(_py, key);
        if key_ptr.is_null() {
            for bits in owned {
                dec_ref_bits(_py, bits);
            }
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
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
        Err(raise_exception::<_>(_py, "MemoryError", "out of memory"))
    } else {
        Ok(MoltObject::from_ptr(dict_ptr).bits())
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_metadata_payload(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let payload = importlib_metadata_payload(&path);
        match alloc_importlib_metadata_payload_dict_bits(_py, &payload) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_metadata_distributions_payload(
    search_paths_bits: u64,
    module_file_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let search_paths =
            match string_sequence_arg_from_bits(_py, search_paths_bits, "search paths") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let payloads = importlib_metadata_distributions_payload(&search_paths, module_file);
        let mut item_bits: Vec<u64> = Vec::with_capacity(payloads.len());
        for payload in &payloads {
            let bits = match alloc_importlib_metadata_payload_dict_bits(_py, payload) {
                Ok(bits) => bits,
                Err(err) => {
                    for value_bits in item_bits {
                        dec_ref_bits(_py, value_bits);
                    }
                    return err;
                }
            };
            item_bits.push(bits);
        }
        let list_ptr = alloc_list(_py, item_bits.as_slice());
        for bits in item_bits {
            dec_ref_bits(_py, bits);
        }
        if list_ptr.is_null() {
            raise_exception::<_>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_metadata_record_payload(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let entries = importlib_metadata_record_payload(&path);
        let mut tuple_bits_vec: Vec<u64> = Vec::with_capacity(entries.len());
        for entry in entries {
            let record_path_bits = match alloc_str_bits(_py, &entry.path) {
                Ok(bits) => bits,
                Err(err) => {
                    for bits in tuple_bits_vec {
                        dec_ref_bits(_py, bits);
                    }
                    return err;
                }
            };
            let hash_bits = match entry.hash.as_deref() {
                Some(value) => match alloc_str_bits(_py, value) {
                    Ok(bits) => bits,
                    Err(err) => {
                        dec_ref_bits(_py, record_path_bits);
                        for bits in tuple_bits_vec {
                            dec_ref_bits(_py, bits);
                        }
                        return err;
                    }
                },
                None => MoltObject::none().bits(),
            };
            let size_bits = match entry.size.as_deref() {
                Some(value) => match alloc_str_bits(_py, value) {
                    Ok(bits) => bits,
                    Err(err) => {
                        if !obj_from_bits(hash_bits).is_none() {
                            dec_ref_bits(_py, hash_bits);
                        }
                        dec_ref_bits(_py, record_path_bits);
                        for bits in tuple_bits_vec {
                            dec_ref_bits(_py, bits);
                        }
                        return err;
                    }
                },
                None => MoltObject::none().bits(),
            };
            let tuple_ptr = alloc_tuple(_py, &[record_path_bits, hash_bits, size_bits]);
            dec_ref_bits(_py, record_path_bits);
            if !obj_from_bits(hash_bits).is_none() {
                dec_ref_bits(_py, hash_bits);
            }
            if !obj_from_bits(size_bits).is_none() {
                dec_ref_bits(_py, size_bits);
            }
            if tuple_ptr.is_null() {
                for bits in tuple_bits_vec {
                    dec_ref_bits(_py, bits);
                }
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            tuple_bits_vec.push(MoltObject::from_ptr(tuple_ptr).bits());
        }
        let list_ptr = alloc_list(_py, tuple_bits_vec.as_slice());
        for bits in tuple_bits_vec {
            dec_ref_bits(_py, bits);
        }
        if list_ptr.is_null() {
            raise_exception::<_>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_metadata_packages_distributions_payload(
    search_paths_bits: u64,
    module_file_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let search_paths =
            match string_sequence_arg_from_bits(_py, search_paths_bits, "search paths") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let payload = importlib_metadata_packages_distributions_payload(&search_paths, module_file);
        let mut pairs: Vec<u64> = Vec::with_capacity(payload.len() * 2);
        let mut owned: Vec<u64> = Vec::with_capacity(payload.len() * 2);
        for (package, providers) in payload {
            let package_bits = match alloc_str_bits(_py, &package) {
                Ok(bits) => bits,
                Err(err) => {
                    for bits in owned {
                        dec_ref_bits(_py, bits);
                    }
                    return err;
                }
            };
            let providers_bits = match alloc_string_list_bits(_py, &providers) {
                Some(bits) => bits,
                None => {
                    dec_ref_bits(_py, package_bits);
                    for bits in owned {
                        dec_ref_bits(_py, bits);
                    }
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
            };
            pairs.push(package_bits);
            pairs.push(providers_bits);
            owned.push(package_bits);
            owned.push(providers_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            raise_exception::<_>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(dict_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_module_from_spec(spec_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        static LOADER_NAME: AtomicU64 = AtomicU64::new(0);
        static CREATE_MODULE_NAME: AtomicU64 = AtomicU64::new(0);
        static SPEC_ATTR_NAME: AtomicU64 = AtomicU64::new(0);
        static MODULE_NAME_NAME: AtomicU64 = AtomicU64::new(0);
        static PARENT_NAME: AtomicU64 = AtomicU64::new(0);
        static SUBMODULE_SEARCH_LOCATIONS_NAME: AtomicU64 = AtomicU64::new(0);
        static ORIGIN_NAME: AtomicU64 = AtomicU64::new(0);
        static CACHED_NAME: AtomicU64 = AtomicU64::new(0);
        static DUNDER_LOADER_NAME: AtomicU64 = AtomicU64::new(0);
        static DUNDER_PACKAGE_NAME: AtomicU64 = AtomicU64::new(0);
        static DUNDER_PATH_NAME: AtomicU64 = AtomicU64::new(0);
        static DUNDER_FILE_NAME: AtomicU64 = AtomicU64::new(0);
        static DUNDER_CACHED_NAME: AtomicU64 = AtomicU64::new(0);

        let loader_name = intern_static_name(_py, &LOADER_NAME, b"loader");
        let create_module_name = intern_static_name(_py, &CREATE_MODULE_NAME, b"create_module");
        let mut loader_bits = MoltObject::none().bits();
        let mut module_bits = MoltObject::none().bits();
        let out = (|| -> Result<u64, u64> {
            if let Some(bits) = getattr_optional_bits(_py, spec_bits, loader_name)? {
                loader_bits = bits;
                if !obj_from_bits(loader_bits).is_none()
                    && let Some(create_module_bits) =
                        importlib_reader_lookup_callable(_py, loader_bits, create_module_name)?
                {
                    let created_bits =
                        unsafe { call_callable1(_py, create_module_bits, spec_bits) };
                    dec_ref_bits(_py, create_module_bits);
                    if exception_pending(_py) {
                        clear_exception(_py);
                        if !obj_from_bits(created_bits).is_none() {
                            dec_ref_bits(_py, created_bits);
                        }
                    } else if !obj_from_bits(created_bits).is_none() {
                        module_bits = created_bits;
                    }
                }
            }

            if obj_from_bits(module_bits).is_none() {
                let spec_name_bits = importlib_required_attribute(
                    _py,
                    spec_bits,
                    &MODULE_NAME_NAME,
                    b"name",
                    "importlib.machinery.ModuleSpec",
                )?;
                module_bits = crate::molt_module_new(spec_name_bits);
                if !obj_from_bits(spec_name_bits).is_none() {
                    dec_ref_bits(_py, spec_name_bits);
                }
                if exception_pending(_py) {
                    return Err(MoltObject::none().bits());
                }
            }

            importlib_set_attr(_py, module_bits, &SPEC_ATTR_NAME, b"__spec__", spec_bits)?;
            importlib_set_attr(
                _py,
                module_bits,
                &DUNDER_LOADER_NAME,
                b"__loader__",
                loader_bits,
            )?;

            let locations_name = intern_static_name(
                _py,
                &SUBMODULE_SEARCH_LOCATIONS_NAME,
                b"submodule_search_locations",
            );
            let locations_bits = getattr_optional_bits(_py, spec_bits, locations_name)?;
            let has_locations = match locations_bits {
                Some(bits) => {
                    let out = !obj_from_bits(bits).is_none();
                    if !obj_from_bits(bits).is_none() {
                        dec_ref_bits(_py, bits);
                    }
                    out
                }
                None => false,
            };
            if has_locations {
                let spec_name_bits = importlib_required_attribute(
                    _py,
                    spec_bits,
                    &MODULE_NAME_NAME,
                    b"name",
                    "importlib.machinery.ModuleSpec",
                )?;
                importlib_set_attr(
                    _py,
                    module_bits,
                    &DUNDER_PACKAGE_NAME,
                    b"__package__",
                    spec_name_bits,
                )?;
                if !obj_from_bits(spec_name_bits).is_none() {
                    dec_ref_bits(_py, spec_name_bits);
                }

                let locations_bits = importlib_required_attribute(
                    _py,
                    spec_bits,
                    &SUBMODULE_SEARCH_LOCATIONS_NAME,
                    b"submodule_search_locations",
                    "importlib.machinery.ModuleSpec",
                )?;
                let path_bits = importlib_list_from_iterable(
                    _py,
                    locations_bits,
                    "spec.submodule_search_locations",
                )?;
                if !obj_from_bits(locations_bits).is_none() {
                    dec_ref_bits(_py, locations_bits);
                }
                importlib_set_attr(_py, module_bits, &DUNDER_PATH_NAME, b"__path__", path_bits)?;
                if !obj_from_bits(path_bits).is_none() {
                    dec_ref_bits(_py, path_bits);
                }
            } else {
                let parent_bits = importlib_required_attribute(
                    _py,
                    spec_bits,
                    &PARENT_NAME,
                    b"parent",
                    "importlib.machinery.ModuleSpec",
                )?;
                importlib_set_attr(
                    _py,
                    module_bits,
                    &DUNDER_PACKAGE_NAME,
                    b"__package__",
                    parent_bits,
                )?;
                if !obj_from_bits(parent_bits).is_none() {
                    dec_ref_bits(_py, parent_bits);
                }
            }

            let origin_name = intern_static_name(_py, &ORIGIN_NAME, b"origin");
            if let Some(origin_bits) = getattr_optional_bits(_py, spec_bits, origin_name)?
                && !obj_from_bits(origin_bits).is_none()
            {
                importlib_set_attr(
                    _py,
                    module_bits,
                    &DUNDER_FILE_NAME,
                    b"__file__",
                    origin_bits,
                )?;
                dec_ref_bits(_py, origin_bits);
            }

            let cached_bits = importlib_required_attribute(
                _py,
                spec_bits,
                &CACHED_NAME,
                b"cached",
                "importlib.machinery.ModuleSpec",
            )?;
            importlib_set_attr(
                _py,
                module_bits,
                &DUNDER_CACHED_NAME,
                b"__cached__",
                cached_bits,
            )?;
            if !obj_from_bits(cached_bits).is_none() {
                dec_ref_bits(_py, cached_bits);
            }
            Ok(module_bits)
        })();

        if !obj_from_bits(loader_bits).is_none() {
            dec_ref_bits(_py, loader_bits);
        }
        match out {
            Ok(bits) => bits,
            Err(err) => {
                if !obj_from_bits(module_bits).is_none() {
                    dec_ref_bits(_py, module_bits);
                }
                err
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_spec_from_loader(
    name_bits: u64,
    loader_bits: u64,
    origin_bits: u64,
    is_package_bits: u64,
    machinery_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        static MODULE_SPEC_NAME: AtomicU64 = AtomicU64::new(0);
        static IS_PACKAGE_NAME: AtomicU64 = AtomicU64::new(0);
        let _name = match string_arg_from_bits(_py, name_bits, "name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut is_package_arg_bits = is_package_bits;
        if obj_from_bits(is_package_bits).is_none() && !obj_from_bits(loader_bits).is_none() {
            let is_package_name = intern_static_name(_py, &IS_PACKAGE_NAME, b"is_package");
            if let Some(call_bits) =
                match importlib_reader_lookup_callable(_py, loader_bits, is_package_name) {
                    Ok(value) => value,
                    Err(bits) => return bits,
                }
            {
                let value_bits = unsafe { call_callable1(_py, call_bits, name_bits) };
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    clear_exception(_py);
                } else {
                    let is_package = is_truthy(_py, obj_from_bits(value_bits));
                    if exception_pending(_py) {
                        clear_exception(_py);
                    } else {
                        is_package_arg_bits = MoltObject::from_bool(is_package).bits();
                    }
                }
                if !obj_from_bits(value_bits).is_none() {
                    dec_ref_bits(_py, value_bits);
                }
            }
        }
        let module_spec_cls_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &MODULE_SPEC_NAME,
            b"ModuleSpec",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let spec_bits = match call_callable_positional(
            _py,
            module_spec_cls_bits,
            &[name_bits, loader_bits, origin_bits, is_package_arg_bits],
        ) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, module_spec_cls_bits);
                return err;
            }
        };
        dec_ref_bits(_py, module_spec_cls_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if let Err(err) = importlib_spec_set_cached_from_origin_if_missing(_py, spec_bits) {
            if !obj_from_bits(spec_bits).is_none() {
                dec_ref_bits(_py, spec_bits);
            }
            return err;
        }
        spec_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_spec_from_file_location(
    name_bits: u64,
    location_bits: u64,
    loader_bits: u64,
    submodule_search_locations_bits: u64,
    machinery_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        static MODULE_SPEC_NAME: AtomicU64 = AtomicU64::new(0);
        static SOURCE_FILE_LOADER_NAME: AtomicU64 = AtomicU64::new(0);
        static SUBMODULE_SEARCH_LOCATIONS_NAME: AtomicU64 = AtomicU64::new(0);

        let _name = match string_arg_from_bits(_py, name_bits, "name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let location_text_bits =
            unsafe { call_callable1(_py, builtin_classes(_py).str, location_bits) };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let path = match string_arg_from_bits(_py, location_text_bits, "location") {
            Ok(value) => value,
            Err(bits) => {
                if !obj_from_bits(location_text_bits).is_none() {
                    dec_ref_bits(_py, location_text_bits);
                }
                return bits;
            }
        };
        if !obj_from_bits(location_text_bits).is_none() {
            dec_ref_bits(_py, location_text_bits);
        }
        let payload = importlib_spec_from_file_location_payload(&path);
        let path_bits = match alloc_str_bits(_py, &payload.path) {
            Ok(bits) => bits,
            Err(err) => return err,
        };

        let mut effective_loader_bits = loader_bits;
        let mut effective_loader_owned = false;
        let mut locations_bits = MoltObject::none().bits();
        let mut locations_owned = false;
        let mut spec_bits = MoltObject::none().bits();
        let out = (|| -> Result<u64, u64> {
            if obj_from_bits(loader_bits).is_none() {
                let loader_cls_bits = importlib_required_attribute(
                    _py,
                    machinery_bits,
                    &SOURCE_FILE_LOADER_NAME,
                    b"SourceFileLoader",
                    "importlib.machinery",
                )?;
                effective_loader_bits =
                    call_callable_positional(_py, loader_cls_bits, &[name_bits, path_bits])?;
                dec_ref_bits(_py, loader_cls_bits);
                if exception_pending(_py) {
                    return Err(MoltObject::none().bits());
                }
                effective_loader_owned = true;
            }

            if !obj_from_bits(submodule_search_locations_bits).is_none() {
                locations_bits = importlib_list_from_iterable(
                    _py,
                    submodule_search_locations_bits,
                    "submodule_search_locations",
                )?;
                locations_owned = true;
            } else if payload.is_package {
                let Some(root) = payload.package_root.as_deref() else {
                    return Err(raise_exception::<_>(
                        _py,
                        "RuntimeError",
                        "invalid importlib spec_from_file_location payload: package_root",
                    ));
                };
                let root_bits = alloc_str_bits(_py, root)?;
                let list_ptr = alloc_list(_py, &[root_bits]);
                if !obj_from_bits(root_bits).is_none() {
                    dec_ref_bits(_py, root_bits);
                }
                if list_ptr.is_null() {
                    return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
                }
                locations_bits = MoltObject::from_ptr(list_ptr).bits();
                locations_owned = true;
            }

            let is_package_bits = MoltObject::from_bool(
                payload.is_package || !obj_from_bits(locations_bits).is_none(),
            )
            .bits();
            let module_spec_cls_bits = importlib_required_attribute(
                _py,
                machinery_bits,
                &MODULE_SPEC_NAME,
                b"ModuleSpec",
                "importlib.machinery",
            )?;
            spec_bits = call_callable_positional(
                _py,
                module_spec_cls_bits,
                &[name_bits, effective_loader_bits, path_bits, is_package_bits],
            )?;
            dec_ref_bits(_py, module_spec_cls_bits);
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }

            if !obj_from_bits(locations_bits).is_none() {
                importlib_set_attr(
                    _py,
                    spec_bits,
                    &SUBMODULE_SEARCH_LOCATIONS_NAME,
                    b"submodule_search_locations",
                    locations_bits,
                )?;
            }
            importlib_spec_set_cached_from_origin_if_missing(_py, spec_bits)?;
            Ok(spec_bits)
        })();

        if !obj_from_bits(path_bits).is_none() {
            dec_ref_bits(_py, path_bits);
        }
        if locations_owned && !obj_from_bits(locations_bits).is_none() {
            dec_ref_bits(_py, locations_bits);
        }
        if effective_loader_owned && !obj_from_bits(effective_loader_bits).is_none() {
            dec_ref_bits(_py, effective_loader_bits);
        }
        match out {
            Ok(bits) => bits,
            Err(err) => {
                if !obj_from_bits(spec_bits).is_none() {
                    dec_ref_bits(_py, spec_bits);
                }
                err
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_set_module_state(
    module_bits: u64,
    module_name_bits: u64,
    loader_bits: u64,
    origin_bits: u64,
    is_package_bits: u64,
    module_package_bits: u64,
    package_root_bits: u64,
    module_spec_cls_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        static SPEC_NAME: AtomicU64 = AtomicU64::new(0);
        static MODULE_NAME_NAME: AtomicU64 = AtomicU64::new(0);
        static DUNDER_PATH_NAME: AtomicU64 = AtomicU64::new(0);
        static SUBMODULE_SEARCH_LOCATIONS_NAME: AtomicU64 = AtomicU64::new(0);

        let _module_name = match string_arg_from_bits(_py, module_name_bits, "module_name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let _origin = match string_arg_from_bits(_py, origin_bits, "origin") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let _module_package = match string_arg_from_bits(_py, module_package_bits, "module_package")
        {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let is_package = is_truthy(_py, obj_from_bits(is_package_bits));
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }

        let mut spec_bits = MoltObject::none().bits();
        let mut spec_owned = false;
        let mut module_path_bits = MoltObject::none().bits();
        let mut module_path_owned = false;
        let mut spec_locations_bits = MoltObject::none().bits();
        let mut spec_locations_owned = false;
        let mut modules_bits = MoltObject::none().bits();
        let mut modules_owned = false;

        let out = (|| -> Result<(), u64> {
            let spec_name = intern_static_name(_py, &SPEC_NAME, b"__spec__");
            if let Some(existing_spec_bits) = getattr_optional_bits(_py, module_bits, spec_name)?
                && !obj_from_bits(existing_spec_bits).is_none()
            {
                spec_bits = existing_spec_bits;
                spec_owned = true;
            }

            if obj_from_bits(spec_bits).is_none() {
                spec_bits = call_callable_positional(
                    _py,
                    module_spec_cls_bits,
                    &[
                        module_name_bits,
                        loader_bits,
                        origin_bits,
                        MoltObject::from_bool(is_package).bits(),
                    ],
                )?;
                if exception_pending(_py) {
                    return Err(MoltObject::none().bits());
                }
                importlib_set_attr(_py, module_bits, &SPEC_NAME, b"__spec__", spec_bits)?;
                spec_owned = true;
            } else {
                let module_name_name = intern_static_name(_py, &MODULE_NAME_NAME, b"name");
                let should_fix_name = match getattr_optional_bits(_py, spec_bits, module_name_name)?
                {
                    Some(bits) => {
                        let is_str = string_obj_to_owned(obj_from_bits(bits)).is_some();
                        if !obj_from_bits(bits).is_none() {
                            dec_ref_bits(_py, bits);
                        }
                        !is_str
                    }
                    None => true,
                };
                if should_fix_name
                    && importlib_set_attr(
                        _py,
                        spec_bits,
                        &MODULE_NAME_NAME,
                        b"name",
                        module_name_bits,
                    )
                    .is_err()
                {
                    clear_exception(_py);
                    return Err(raise_exception::<_>(
                        _py,
                        "RuntimeError",
                        "invalid module spec name state",
                    ));
                }
                importlib_spec_set_loader_origin(_py, spec_bits, loader_bits, origin_bits)?;
            }

            importlib_module_set_core_state(
                _py,
                module_bits,
                loader_bits,
                origin_bits,
                module_package_bits,
            )?;

            if is_package {
                importlib_require_package_root_bits(_py, package_root_bits)?;
                module_path_bits = importlib_single_item_list_bits(_py, package_root_bits)?;
                module_path_owned = true;
                importlib_set_attr(
                    _py,
                    module_bits,
                    &DUNDER_PATH_NAME,
                    b"__path__",
                    module_path_bits,
                )?;

                let locations_name = intern_static_name(
                    _py,
                    &SUBMODULE_SEARCH_LOCATIONS_NAME,
                    b"submodule_search_locations",
                );
                let should_set_locations =
                    match getattr_optional_bits(_py, spec_bits, locations_name)? {
                        Some(bits) => {
                            let is_none = obj_from_bits(bits).is_none();
                            if !is_none {
                                dec_ref_bits(_py, bits);
                            }
                            is_none
                        }
                        None => true,
                    };
                if should_set_locations {
                    spec_locations_bits = importlib_single_item_list_bits(_py, package_root_bits)?;
                    spec_locations_owned = true;
                    importlib_set_attr(
                        _py,
                        spec_bits,
                        &SUBMODULE_SEARCH_LOCATIONS_NAME,
                        b"submodule_search_locations",
                        spec_locations_bits,
                    )?;
                }
            }

            modules_bits = importlib_runtime_modules_bits(_py)?;
            modules_owned = true;
            let Some(modules_ptr) = obj_from_bits(modules_bits).as_ptr() else {
                return Err(importlib_modules_runtime_error(_py));
            };
            unsafe {
                dict_set_in_place(_py, modules_ptr, module_name_bits, module_bits);
            }
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }
            Ok(())
        })();

        if modules_owned && !obj_from_bits(modules_bits).is_none() {
            dec_ref_bits(_py, modules_bits);
        }
        if spec_locations_owned && !obj_from_bits(spec_locations_bits).is_none() {
            dec_ref_bits(_py, spec_locations_bits);
        }
        if module_path_owned && !obj_from_bits(module_path_bits).is_none() {
            dec_ref_bits(_py, module_path_bits);
        }
        if spec_owned && !obj_from_bits(spec_bits).is_none() {
            dec_ref_bits(_py, spec_bits);
        }
        match out {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_stabilize_module_state(
    module_bits: u64,
    loader_bits: u64,
    origin_bits: u64,
    is_package_bits: u64,
    module_package_bits: u64,
    package_root_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        static SPEC_NAME: AtomicU64 = AtomicU64::new(0);
        static DUNDER_PATH_NAME: AtomicU64 = AtomicU64::new(0);
        static SUBMODULE_SEARCH_LOCATIONS_NAME: AtomicU64 = AtomicU64::new(0);

        let _origin = match string_arg_from_bits(_py, origin_bits, "origin") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let _module_package = match string_arg_from_bits(_py, module_package_bits, "module_package")
        {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let is_package = is_truthy(_py, obj_from_bits(is_package_bits));
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }

        let mut spec_bits = MoltObject::none().bits();
        let mut spec_owned = false;
        let mut module_path_bits = MoltObject::none().bits();
        let mut module_path_owned = false;
        let out = (|| -> Result<(), u64> {
            let spec_name = intern_static_name(_py, &SPEC_NAME, b"__spec__");
            if let Some(existing_spec_bits) = getattr_optional_bits(_py, module_bits, spec_name)?
                && !obj_from_bits(existing_spec_bits).is_none()
            {
                spec_bits = existing_spec_bits;
                spec_owned = true;
                importlib_spec_set_loader_origin(_py, spec_bits, loader_bits, origin_bits)?;
            }

            importlib_module_set_core_state(
                _py,
                module_bits,
                loader_bits,
                origin_bits,
                module_package_bits,
            )?;

            if is_package {
                importlib_require_package_root_bits(_py, package_root_bits)?;
                let dunder_path_name = intern_static_name(_py, &DUNDER_PATH_NAME, b"__path__");
                if let Some(existing_path_bits) =
                    getattr_optional_bits(_py, module_bits, dunder_path_name)?
                {
                    if importlib_is_str_list_bits(existing_path_bits) {
                        module_path_bits = existing_path_bits;
                        module_path_owned = true;
                    } else if !obj_from_bits(existing_path_bits).is_none() {
                        dec_ref_bits(_py, existing_path_bits);
                    }
                }
                if obj_from_bits(module_path_bits).is_none() {
                    module_path_bits = importlib_single_item_list_bits(_py, package_root_bits)?;
                    module_path_owned = true;
                    importlib_set_attr(
                        _py,
                        module_bits,
                        &DUNDER_PATH_NAME,
                        b"__path__",
                        module_path_bits,
                    )?;
                }

                if !obj_from_bits(spec_bits).is_none() {
                    let locations_name = intern_static_name(
                        _py,
                        &SUBMODULE_SEARCH_LOCATIONS_NAME,
                        b"submodule_search_locations",
                    );
                    let should_set_locations =
                        match getattr_optional_bits(_py, spec_bits, locations_name)? {
                            Some(bits) => {
                                let has_valid_locations = importlib_is_str_list_bits(bits);
                                if !obj_from_bits(bits).is_none() {
                                    dec_ref_bits(_py, bits);
                                }
                                !has_valid_locations
                            }
                            None => true,
                        };
                    if should_set_locations {
                        let locations_bits =
                            importlib_list_from_iterable(_py, module_path_bits, "module.__path__")?;
                        let set_out = importlib_set_attr(
                            _py,
                            spec_bits,
                            &SUBMODULE_SEARCH_LOCATIONS_NAME,
                            b"submodule_search_locations",
                            locations_bits,
                        );
                        if !obj_from_bits(locations_bits).is_none() {
                            dec_ref_bits(_py, locations_bits);
                        }
                        set_out?;
                    }
                }
            } else {
                let dunder_path_name = intern_static_name(_py, &DUNDER_PATH_NAME, b"__path__");
                if let Some(module_path_attr_bits) =
                    getattr_optional_bits(_py, module_bits, dunder_path_name)?
                {
                    let should_delete = match obj_from_bits(module_path_attr_bits).as_ptr() {
                        Some(path_ptr) => unsafe { object_type_id(path_ptr) == TYPE_ID_OBJECT },
                        None => false,
                    };
                    if should_delete {
                        let result_bits = crate::molt_object_delattr(module_bits, dunder_path_name);
                        if !obj_from_bits(result_bits).is_none() {
                            dec_ref_bits(_py, result_bits);
                        }
                        if exception_pending(_py) {
                            clear_exception(_py);
                        }
                    }
                    if !obj_from_bits(module_path_attr_bits).is_none() {
                        dec_ref_bits(_py, module_path_attr_bits);
                    }
                }
            }
            Ok(())
        })();

        if module_path_owned && !obj_from_bits(module_path_bits).is_none() {
            dec_ref_bits(_py, module_path_bits);
        }
        if spec_owned && !obj_from_bits(spec_bits).is_none() {
            dec_ref_bits(_py, spec_bits);
        }
        match out {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_spec_from_file_location_payload(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let payload = importlib_spec_from_file_location_payload(&path);
        let path_bits = match alloc_str_bits(_py, &payload.path) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let package_root_bits = match payload.package_root.as_deref() {
            Some(root) => match alloc_str_bits(_py, root) {
                Ok(bits) => bits,
                Err(err) => {
                    dec_ref_bits(_py, path_bits);
                    return err;
                }
            },
            None => MoltObject::none().bits(),
        };
        let is_package_bits = MoltObject::from_bool(payload.is_package).bits();
        let keys_and_values: [(&[u8], u64); 3] = [
            (b"path", path_bits),
            (b"is_package", is_package_bits),
            (b"package_root", package_root_bits),
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
pub extern "C" fn molt_runpy_resolve_path(path_bits: u64, module_file_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let abs_path = bootstrap_resolve_abspath(&path, module_file);
        let is_file = std::fs::metadata(&abs_path)
            .map(|metadata| metadata.is_file())
            .unwrap_or(false);

        let abs_path_bits = match alloc_str_bits(_py, &abs_path) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let is_file_bits = MoltObject::from_bool(is_file).bits();
        let keys_and_values: [(&[u8], u64); 2] =
            [(b"abspath", abs_path_bits), (b"is_file", is_file_bits)];
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
