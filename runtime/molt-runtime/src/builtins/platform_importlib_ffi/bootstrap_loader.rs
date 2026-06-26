use super::*;

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_bootstrap_path(module_file_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let spec_is_package = is_truthy(_py, obj_from_bits(spec_is_package_bits));
        let resolution = match importlib_extension_loader_resolution_checked(
            _py,
            &module_name,
            &path,
            spec_is_package,
        ) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
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
    crate::with_gil_entry_nopanic!(_py, {
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

pub(super) fn importlib_source_exec_payload_checked(
    _py: &PyToken<'_>,
    module_name: &str,
    path: &str,
    spec_is_package: bool,
) -> Result<ImportlibSourceExecPayload, u64> {
    let allowed = has_capability(_py, "fs.read");
    audit_capability_decision(
        "importlib.source_exec_payload",
        "fs.read",
        AuditArgs::None,
        allowed,
    );
    if !allowed {
        return Err(raise_exception::<_>(
            _py,
            "PermissionError",
            "missing fs.read capability",
        ));
    }
    importlib_source_exec_payload(module_name, path, spec_is_package)
        .map_err(|err| raise_importlib_io_error(_py, err))
}

pub(super) fn importlib_zip_source_exec_payload_checked(
    _py: &PyToken<'_>,
    module_name: &str,
    archive_path: &str,
    inner_path: &str,
    spec_is_package: bool,
) -> Result<ImportlibZipSourceExecPayload, u64> {
    let allowed = has_capability(_py, "fs.read");
    audit_capability_decision(
        "importlib.zip.source_exec_payload",
        "fs.read",
        AuditArgs::None,
        allowed,
    );
    if !allowed {
        return Err(raise_exception::<_>(
            _py,
            "PermissionError",
            "missing fs.read capability",
        ));
    }
    importlib_zip_source_exec_payload(module_name, archive_path, inner_path, spec_is_package)
        .map_err(|err| raise_importlib_io_error(_py, err))
}

pub(super) fn importlib_extension_loader_resolution_checked(
    _py: &PyToken<'_>,
    module_name: &str,
    path: &str,
    spec_is_package: bool,
) -> Result<SourceLoaderResolution, u64> {
    let allowed = has_capability(_py, "fs.read");
    audit_capability_decision(
        "importlib.extension_loader_payload",
        "fs.read",
        AuditArgs::None,
        allowed,
    );
    if !allowed {
        return Err(raise_exception::<_>(
            _py,
            "PermissionError",
            "missing fs.read capability",
        ));
    }
    match importlib_path_is_file(_py, path) {
        Ok(true) => {}
        Ok(false) => {
            return Err(raise_exception::<_>(
                _py,
                "ImportError",
                "extension module path must point to a file",
            ));
        }
        Err(bits) => return Err(bits),
    }
    importlib_require_extension_metadata(_py, module_name, path)?;
    Ok(extension_loader_resolution(
        module_name,
        path,
        spec_is_package,
    ))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_source_exec_payload(
    module_name_bits: u64,
    path_bits: u64,
    spec_is_package_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let spec_is_package = is_truthy(_py, obj_from_bits(spec_is_package_bits));
        let payload = match importlib_source_exec_payload_checked(
            _py,
            &module_name,
            &path,
            spec_is_package,
        ) {
            Ok(value) => value,
            Err(bits) => return bits,
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
    crate::with_gil_entry_nopanic!(_py, {
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
        let payload = match importlib_zip_source_exec_payload_checked(
            _py,
            &module_name,
            &archive_path,
            &inner_path,
            spec_is_package,
        ) {
            Ok(value) => value,
            Err(bits) => return bits,
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

pub(super) fn importlib_exec_extension_impl(
    _py: &PyToken<'_>,
    namespace_ptr: *mut u8,
    module_name: &str,
    path: &str,
) -> Result<(), u64> {
    let allowed = has_capability(_py, "fs.read");
    audit_capability_decision(
        "importlib.exec.extension",
        "fs.read",
        AuditArgs::None,
        allowed,
    );
    if !allowed {
        return Err(raise_exception::<_>(
            _py,
            "PermissionError",
            "missing fs.read capability",
        ));
    }
    match importlib_path_is_file(_py, path) {
        Ok(true) => {}
        Ok(false) => {
            return Err(raise_exception::<_>(
                _py,
                "ImportError",
                "extension module path must point to a file",
            ));
        }
        Err(bits) => return Err(bits),
    }
    let ext_allowed =
        has_capability(_py, "module.extension.exec") || has_capability(_py, "module.exec");
    audit_capability_decision(
        "importlib.exec.extension.module",
        "module.extension.exec",
        AuditArgs::None,
        ext_allowed,
    );
    if !ext_allowed {
        return Err(raise_exception::<_>(
            _py,
            "PermissionError",
            "missing module.extension.exec capability",
        ));
    }
    importlib_require_extension_metadata(_py, module_name, path)?;
    let shim_candidates = importlib_extension_shim_candidates(module_name, path);
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
                        module_name,
                        candidate,
                    ) {
                        if restricted_error.is_none() {
                            restricted_error = Some(message);
                        }
                        continue;
                    }
                    return Err(err);
                }
                return Ok(());
            }
            Ok(false) => continue,
            Err(bits) => return Err(bits),
        }
    }
    if let Some(message) = restricted_error {
        return Err(raise_exception::<_>(_py, "ImportError", &message));
    }
    // -- Native C extension loading via dlopen --
    #[cfg(all(feature = "cext_loader", not(target_arch = "wasm32")))]
    {
        match cext_loader_dlopen(_py, namespace_ptr, module_name, path) {
            Ok(()) => return Ok(()),
            Err(msg) => {
                return Err(raise_exception::<_>(
                    _py,
                    "ImportError",
                    &format!("failed to load C extension {module_name:?} from {path:?}: {msg}"),
                ));
            }
        }
    }
    #[allow(unreachable_code)]
    Err(importlib_extension_exec_unavailable(
        _py,
        module_name,
        path,
        "extension",
        &shim_candidates,
    ))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_exec_extension(
    namespace_bits: u64,
    module_name_bits: u64,
    path_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
        match importlib_exec_extension_impl(_py, namespace_ptr, &module_name, &path) {
            Ok(()) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

pub(super) fn importlib_exec_sourceless_impl(
    _py: &PyToken<'_>,
    namespace_ptr: *mut u8,
    module_name: &str,
    path: &str,
) -> Result<(), u64> {
    let allowed = has_capability(_py, "fs.read");
    audit_capability_decision(
        "importlib.exec.sourceless",
        "fs.read",
        AuditArgs::None,
        allowed,
    );
    if !allowed {
        return Err(raise_exception::<_>(
            _py,
            "PermissionError",
            "missing fs.read capability",
        ));
    }
    match importlib_path_is_file(_py, path) {
        Ok(true) => {}
        Ok(false) => {
            return Err(raise_exception::<_>(
                _py,
                "ImportError",
                "sourceless module path must point to a file",
            ));
        }
        Err(bits) => return Err(bits),
    }
    let bc_allowed =
        has_capability(_py, "module.bytecode.exec") || has_capability(_py, "module.exec");
    audit_capability_decision(
        "importlib.exec.sourceless.module",
        "module.bytecode.exec",
        AuditArgs::None,
        bc_allowed,
    );
    if !bc_allowed {
        return Err(raise_exception::<_>(
            _py,
            "PermissionError",
            "missing module.bytecode.exec capability",
        ));
    }
    let source_candidates = importlib_sourceless_source_candidates(module_name, path);
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
                        module_name,
                        candidate,
                    ) {
                        if restricted_error.is_none() {
                            restricted_error = Some(message);
                        }
                        continue;
                    }
                    return Err(err);
                }
                return Ok(());
            }
            Ok(false) => continue,
            Err(bits) => return Err(bits),
        }
    }
    if let Some(message) = restricted_error {
        return Err(raise_exception::<_>(_py, "ImportError", &message));
    }
    Err(importlib_extension_exec_unavailable(
        _py,
        module_name,
        path,
        "sourceless",
        &source_candidates,
    ))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_exec_sourceless(
    namespace_bits: u64,
    module_name_bits: u64,
    path_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
        match importlib_exec_sourceless_impl(_py, namespace_ptr, &module_name, &path) {
            Ok(()) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

pub(super) struct ImportlibLoaderExecContext {
    module_name_bits: u64,
    module_name: String,
    spec_is_package: bool,
}

pub(super) enum ImportlibLoaderExecBody {
    RestrictedSource(Vec<u8>),
    Extension,
    Sourceless,
}

pub(super) struct ImportlibLoaderExecState {
    origin: String,
    is_package: bool,
    module_package: String,
    package_root: Option<String>,
    body: ImportlibLoaderExecBody,
}

pub(super) fn importlib_loader_exec_context(
    _py: &PyToken<'_>,
    module_bits: u64,
    loader_bits: u64,
) -> Result<ImportlibLoaderExecContext, u64> {
    let module_name_bits = importlib_coerce_module_name_bits(
        _py,
        module_bits,
        loader_bits,
        MoltObject::none().bits(),
    )?;
    let Some(module_name) = string_obj_to_owned(obj_from_bits(module_name_bits)) else {
        if !obj_from_bits(module_name_bits).is_none() {
            dec_ref_bits(_py, module_name_bits);
        }
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "module name must be str",
        ));
    };
    let spec_is_package = match importlib_module_spec_is_package_bits(_py, module_bits) {
        Ok(value) => value,
        Err(bits) => {
            if !obj_from_bits(module_name_bits).is_none() {
                dec_ref_bits(_py, module_name_bits);
            }
            return Err(bits);
        }
    };
    Ok(ImportlibLoaderExecContext {
        module_name_bits,
        module_name,
        spec_is_package,
    })
}

pub(super) fn importlib_loader_exec_context_release(
    _py: &PyToken<'_>,
    ctx: &ImportlibLoaderExecContext,
) {
    if !obj_from_bits(ctx.module_name_bits).is_none() {
        dec_ref_bits(_py, ctx.module_name_bits);
    }
}

pub(super) fn importlib_loader_exec_module_apply(
    _py: &PyToken<'_>,
    loader_bits: u64,
    module_bits: u64,
    module_spec_cls_bits: u64,
    ctx: &ImportlibLoaderExecContext,
    state: ImportlibLoaderExecState,
) -> Result<(), u64> {
    let origin_bits = alloc_str_bits(_py, &state.origin)?;
    let mut module_package_bits = MoltObject::none().bits();
    let mut package_root_bits = MoltObject::none().bits();
    let out = (|| -> Result<(), u64> {
        module_package_bits = alloc_str_bits(_py, &state.module_package)?;
        package_root_bits = match state.package_root.as_deref() {
            Some(root) => alloc_str_bits(_py, root)?,
            None => MoltObject::none().bits(),
        };

        importlib_set_module_state_impl(
            _py,
            ImportlibModuleStateArgs {
                module_bits,
                module_name_bits: ctx.module_name_bits,
                loader_bits,
                origin_bits,
                is_package: state.is_package,
                module_package_bits,
                package_root_bits,
                module_spec_cls_bits,
            },
        )?;

        let namespace_ptr = importlib_module_dict_ptr_for_state(_py, module_bits)?;
        match &state.body {
            ImportlibLoaderExecBody::RestrictedSource(source_bytes) => {
                let source = importlib_decode_source_text(source_bytes);
                unsafe {
                    crate::builtins::modules::runpy_exec_restricted_source(
                        _py,
                        namespace_ptr,
                        &source,
                        &state.origin,
                    )?;
                }
            }
            ImportlibLoaderExecBody::Extension => {
                importlib_exec_extension_impl(_py, namespace_ptr, &ctx.module_name, &state.origin)?;
            }
            ImportlibLoaderExecBody::Sourceless => {
                importlib_exec_sourceless_impl(
                    _py,
                    namespace_ptr,
                    &ctx.module_name,
                    &state.origin,
                )?;
            }
        }

        Ok(())
    })();

    if !obj_from_bits(package_root_bits).is_none() {
        dec_ref_bits(_py, package_root_bits);
    }
    if !obj_from_bits(module_package_bits).is_none() {
        dec_ref_bits(_py, module_package_bits);
    }
    if !obj_from_bits(origin_bits).is_none() {
        dec_ref_bits(_py, origin_bits);
    }
    out
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_sourcefileloader_exec_module(
    loader_bits: u64,
    module_bits: u64,
    path_bits: u64,
    module_spec_cls_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let ctx = match importlib_loader_exec_context(_py, module_bits, loader_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let out = (|| -> Result<(), u64> {
            let payload = importlib_source_exec_payload_checked(
                _py,
                &ctx.module_name,
                &path,
                ctx.spec_is_package,
            )?;
            importlib_loader_exec_module_apply(
                _py,
                loader_bits,
                module_bits,
                module_spec_cls_bits,
                &ctx,
                ImportlibLoaderExecState {
                    origin: path,
                    is_package: payload.is_package,
                    module_package: payload.module_package,
                    package_root: payload.package_root,
                    body: ImportlibLoaderExecBody::RestrictedSource(payload.source),
                },
            )
        })();
        importlib_loader_exec_context_release(_py, &ctx);
        match out {
            Ok(()) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_zip_source_loader_exec_module(
    loader_bits: u64,
    module_bits: u64,
    archive_path_bits: u64,
    inner_path_bits: u64,
    module_spec_cls_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let archive_path = match string_arg_from_bits(_py, archive_path_bits, "archive path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let inner_path = match string_arg_from_bits(_py, inner_path_bits, "inner path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let ctx = match importlib_loader_exec_context(_py, module_bits, loader_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let out = (|| -> Result<(), u64> {
            let payload = importlib_zip_source_exec_payload_checked(
                _py,
                &ctx.module_name,
                &archive_path,
                &inner_path,
                ctx.spec_is_package,
            )?;
            importlib_loader_exec_module_apply(
                _py,
                loader_bits,
                module_bits,
                module_spec_cls_bits,
                &ctx,
                ImportlibLoaderExecState {
                    origin: payload.origin,
                    is_package: payload.is_package,
                    module_package: payload.module_package,
                    package_root: payload.package_root,
                    body: ImportlibLoaderExecBody::RestrictedSource(payload.source),
                },
            )
        })();
        importlib_loader_exec_context_release(_py, &ctx);
        match out {
            Ok(()) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_extension_loader_exec_module(
    loader_bits: u64,
    module_bits: u64,
    path_bits: u64,
    module_spec_cls_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let ctx = match importlib_loader_exec_context(_py, module_bits, loader_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let out = (|| -> Result<(), u64> {
            let resolution = importlib_extension_loader_resolution_checked(
                _py,
                &ctx.module_name,
                &path,
                ctx.spec_is_package,
            )?;
            importlib_loader_exec_module_apply(
                _py,
                loader_bits,
                module_bits,
                module_spec_cls_bits,
                &ctx,
                ImportlibLoaderExecState {
                    origin: path,
                    is_package: resolution.is_package,
                    module_package: resolution.module_package,
                    package_root: resolution.package_root,
                    body: ImportlibLoaderExecBody::Extension,
                },
            )
        })();
        importlib_loader_exec_context_release(_py, &ctx);
        match out {
            Ok(()) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_sourceless_loader_exec_module(
    loader_bits: u64,
    module_bits: u64,
    path_bits: u64,
    module_spec_cls_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if importlib_is_archive_member_path(&path) {
            return raise_exception::<_>(_py, "NotADirectoryError", &path);
        }
        let ctx = match importlib_loader_exec_context(_py, module_bits, loader_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let out = {
            let resolution =
                sourceless_loader_resolution(&ctx.module_name, &path, ctx.spec_is_package);
            importlib_loader_exec_module_apply(
                _py,
                loader_bits,
                module_bits,
                module_spec_cls_bits,
                &ctx,
                ImportlibLoaderExecState {
                    origin: path,
                    is_package: resolution.is_package,
                    module_package: resolution.module_package,
                    package_root: resolution.package_root,
                    body: ImportlibLoaderExecBody::Sourceless,
                },
            )
        };
        importlib_loader_exec_context_release(_py, &ctx);
        match out {
            Ok(()) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_linecache_loader_get_source(loader_bits: u64, module_name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
        match importlib_module_spec_is_package_bits(_py, module_bits) {
            Ok(value) => MoltObject::from_bool(value).bits(),
            Err(bits) => bits,
        }
    })
}

pub(super) fn importlib_coerce_module_name_bits(
    _py: &PyToken<'_>,
    module_bits: u64,
    loader_bits: u64,
    spec_bits: u64,
) -> Result<u64, u64> {
    let module_name_name = intern_runtime_static_name(_py, b"__name__");
    if let Some(module_name_bits) = getattr_optional_bits(_py, module_bits, module_name_name)? {
        if string_obj_to_owned(obj_from_bits(module_name_bits)).is_some() {
            return Ok(module_name_bits);
        }
        if !obj_from_bits(module_name_bits).is_none() {
            dec_ref_bits(_py, module_name_bits);
        }
    }

    let mut module_spec_bits = spec_bits;
    let mut module_spec_owned = false;
    if obj_from_bits(module_spec_bits).is_none() {
        let spec_name = intern_runtime_static_name(_py, b"__spec__");
        if let Some(bits) = getattr_optional_bits(_py, module_bits, spec_name)? {
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
                    return Err(bits);
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
            return Ok(spec_name_bits);
        }
        if !obj_from_bits(spec_name_bits).is_none() {
            dec_ref_bits(_py, spec_name_bits);
        }
    }

    if module_spec_owned && !obj_from_bits(module_spec_bits).is_none() {
        dec_ref_bits(_py, module_spec_bits);
    }

    let loader_name = intern_runtime_static_name(_py, b"name");
    if let Some(loader_name_bits) = getattr_optional_bits(_py, loader_bits, loader_name)? {
        if string_obj_to_owned(obj_from_bits(loader_name_bits)).is_some() {
            let set_bits =
                crate::molt_object_setattr(module_bits, module_name_name, loader_name_bits);
            if !obj_from_bits(set_bits).is_none() {
                dec_ref_bits(_py, set_bits);
            }
            if exception_pending(_py) {
                clear_exception(_py);
            }
            return Ok(loader_name_bits);
        }
        if !obj_from_bits(loader_name_bits).is_none() {
            dec_ref_bits(_py, loader_name_bits);
        }
    }

    Err(raise_exception::<_>(
        _py,
        "TypeError",
        "module name must be str",
    ))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_coerce_module_name(
    module_bits: u64,
    loader_bits: u64,
    spec_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match importlib_coerce_module_name_bits(_py, module_bits, loader_bits, spec_bits) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

pub(in super::super) fn importlib_coerce_search_paths_values(
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

pub(super) fn importlib_alloc_string_tuple_bits(
    _py: &PyToken<'_>,
    values: &[String],
) -> Result<u64, u64> {
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

pub(super) fn importlib_finder_signature_tuple_bits(
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

pub(super) fn importlib_path_importer_cache_signature_tuple_bits(
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(importlib_is_archive_member_path(&path)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_package_root_from_origin(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
pub extern "C" fn molt_traceback_exception_suppress_context(value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.zip.read_entry",
            "fs.read",
            AuditArgs::None,
            allowed,
        );
        if !allowed {
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
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision("importlib.read_file", "fs.read", AuditArgs::None, allowed);
        if !allowed {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
        inc_ref_bits(_py, path_bits);
        path_bits
    })
}
