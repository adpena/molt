use super::*;

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_path_payload(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.resources.path_payload",
            "fs.read",
            AuditArgs::None,
            allowed,
        );
        if !allowed {
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
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.resources.package_info",
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
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.resources.open_resource_bytes_from_package",
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
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.resources.open_resource_bytes_from_package_parts",
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
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.resources.read_text_from_package",
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
        let decode_name = intern_runtime_static_name(_py, b"decode");
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
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.resources.read_text_from_package_parts",
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
        let decode_name = intern_runtime_static_name(_py, b"decode");
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
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.resources.contents_from_package",
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
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.resources.contents_from_package_parts",
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
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.resources.is_resource_from_package",
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
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.resources.is_resource_from_package_parts",
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
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.resources.resource_path_from_package",
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
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.resources.resource_path_from_package_parts",
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, { MoltObject::from_bool(false).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_module_name(
    module_bits: u64,
    fallback_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.resources.files_payload",
            "fs.read",
            AuditArgs::None,
            allowed,
        );
        if !allowed {
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
    crate::with_gil_entry_nopanic!(_py, {
        match importlib_reader_files_traversable_bits(_py, reader_bits) {
            Ok(Some(bits)) => bits,
            Ok(None) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_reader_roots(reader_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
