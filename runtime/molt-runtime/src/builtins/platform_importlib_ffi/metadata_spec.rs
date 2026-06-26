use super::*;

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_metadata_dist_paths(
    search_paths_bits: u64,
    module_file_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.metadata.dist_paths",
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
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.metadata.entry_points_payload",
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
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.metadata.entry_points_select_payload",
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
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.metadata.entry_points_filter_payload",
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
    crate::with_gil_entry_nopanic!(_py, {
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

pub(super) fn alloc_importlib_metadata_payload_dict_bits(
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
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.metadata.payload",
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
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.metadata.distributions_payload",
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
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.metadata.record_payload",
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
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.metadata.packages_distributions_payload",
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
    crate::with_gil_entry_nopanic!(_py, { importlib_module_from_spec_impl(_py, spec_bits) })
}

pub(in super::super) fn importlib_module_from_spec_impl(_py: &PyToken<'_>, spec_bits: u64) -> u64 {
    let loader_name = intern_runtime_static_name(_py, b"loader");
    let create_module_name = intern_runtime_static_name(_py, b"create_module");
    let mut loader_bits = MoltObject::none().bits();
    let mut module_bits = MoltObject::none().bits();
    let out = (|| -> Result<u64, u64> {
        if let Some(bits) = getattr_optional_bits(_py, spec_bits, loader_name)? {
            loader_bits = bits;
            if !obj_from_bits(loader_bits).is_none()
                && let Some(create_module_bits) =
                    importlib_reader_lookup_callable(_py, loader_bits, create_module_name)?
            {
                let created_bits = unsafe { call_callable1(_py, create_module_bits, spec_bits) };
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
                runtime_static_name_slot(_py, b"name"),
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

        importlib_set_attr(
            _py,
            module_bits,
            runtime_static_name_slot(_py, b"__spec__"),
            b"__spec__",
            spec_bits,
        )?;
        importlib_set_attr(
            _py,
            module_bits,
            runtime_static_name_slot(_py, b"__loader__"),
            b"__loader__",
            loader_bits,
        )?;

        let locations_name = intern_static_name(
            _py,
            runtime_static_name_slot(_py, b"submodule_search_locations"),
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
                runtime_static_name_slot(_py, b"name"),
                b"name",
                "importlib.machinery.ModuleSpec",
            )?;
            importlib_set_attr(
                _py,
                module_bits,
                runtime_static_name_slot(_py, b"__package__"),
                b"__package__",
                spec_name_bits,
            )?;
            if !obj_from_bits(spec_name_bits).is_none() {
                dec_ref_bits(_py, spec_name_bits);
            }

            let locations_bits = importlib_required_attribute(
                _py,
                spec_bits,
                runtime_static_name_slot(_py, b"submodule_search_locations"),
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
            importlib_set_attr(
                _py,
                module_bits,
                runtime_static_name_slot(_py, b"__path__"),
                b"__path__",
                path_bits,
            )?;
            if !obj_from_bits(path_bits).is_none() {
                dec_ref_bits(_py, path_bits);
            }
        } else {
            let parent_bits = importlib_required_attribute(
                _py,
                spec_bits,
                runtime_static_name_slot(_py, b"parent"),
                b"parent",
                "importlib.machinery.ModuleSpec",
            )?;
            importlib_set_attr(
                _py,
                module_bits,
                runtime_static_name_slot(_py, b"__package__"),
                b"__package__",
                parent_bits,
            )?;
            if !obj_from_bits(parent_bits).is_none() {
                dec_ref_bits(_py, parent_bits);
            }
        }

        let origin_name = intern_runtime_static_name(_py, b"origin");
        if let Some(origin_bits) = getattr_optional_bits(_py, spec_bits, origin_name)?
            && !obj_from_bits(origin_bits).is_none()
        {
            importlib_set_attr(
                _py,
                module_bits,
                runtime_static_name_slot(_py, b"__file__"),
                b"__file__",
                origin_bits,
            )?;
            dec_ref_bits(_py, origin_bits);
        }

        let cached_bits = importlib_required_attribute(
            _py,
            spec_bits,
            runtime_static_name_slot(_py, b"cached"),
            b"cached",
            "importlib.machinery.ModuleSpec",
        )?;
        importlib_set_attr(
            _py,
            module_bits,
            runtime_static_name_slot(_py, b"__cached__"),
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
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_spec_from_loader(
    name_bits: u64,
    loader_bits: u64,
    origin_bits: u64,
    is_package_bits: u64,
    machinery_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let _name = match string_arg_from_bits(_py, name_bits, "name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut is_package_arg_bits = is_package_bits;
        if obj_from_bits(is_package_bits).is_none() && !obj_from_bits(loader_bits).is_none() {
            let is_package_name = intern_runtime_static_name(_py, b"is_package");
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
            runtime_static_name_slot(_py, b"ModuleSpec"),
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
    crate::with_gil_entry_nopanic!(_py, {
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
                    runtime_static_name_slot(_py, b"SourceFileLoader"),
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
                runtime_static_name_slot(_py, b"ModuleSpec"),
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
                    runtime_static_name_slot(_py, b"submodule_search_locations"),
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

pub(super) fn importlib_set_module_state_impl(
    _py: &PyToken<'_>,
    args: ImportlibModuleStateArgs,
) -> Result<(), u64> {
    let ImportlibModuleStateArgs {
        module_bits,
        module_name_bits,
        loader_bits,
        origin_bits,
        is_package,
        module_package_bits,
        package_root_bits,
        module_spec_cls_bits,
    } = args;
    let mut spec_bits = MoltObject::none().bits();
    let mut spec_owned = false;
    let mut module_path_bits = MoltObject::none().bits();
    let mut module_path_owned = false;
    let mut spec_locations_bits = MoltObject::none().bits();
    let mut spec_locations_owned = false;
    let mut modules_bits = MoltObject::none().bits();
    let mut modules_owned = false;

    let out = (|| -> Result<(), u64> {
        let spec_name = intern_runtime_static_name(_py, b"__spec__");
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
            importlib_set_attr(
                _py,
                module_bits,
                runtime_static_name_slot(_py, b"__spec__"),
                b"__spec__",
                spec_bits,
            )?;
            spec_owned = true;
        } else {
            let module_name_name = intern_runtime_static_name(_py, b"name");
            let should_fix_name = match getattr_optional_bits(_py, spec_bits, module_name_name)? {
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
                    runtime_static_name_slot(_py, b"name"),
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
                runtime_static_name_slot(_py, b"__path__"),
                b"__path__",
                module_path_bits,
            )?;

            let locations_name = intern_static_name(
                _py,
                runtime_static_name_slot(_py, b"submodule_search_locations"),
                b"submodule_search_locations",
            );
            let should_set_locations = match getattr_optional_bits(_py, spec_bits, locations_name)?
            {
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
                    runtime_static_name_slot(_py, b"submodule_search_locations"),
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
    out
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
    crate::with_gil_entry_nopanic!(_py, {
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

        match importlib_set_module_state_impl(
            _py,
            ImportlibModuleStateArgs {
                module_bits,
                module_name_bits,
                loader_bits,
                origin_bits,
                is_package,
                module_package_bits,
                package_root_bits,
                module_spec_cls_bits,
            },
        ) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => err,
        }
    })
}

pub(super) fn importlib_module_dict_ptr_for_state(
    _py: &PyToken<'_>,
    module_bits: u64,
) -> Result<*mut u8, u64> {
    let Some(module_ptr) = obj_from_bits(module_bits).as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "module state expects module object",
        ));
    };
    if unsafe { object_type_id(module_ptr) } != TYPE_ID_MODULE {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "module state expects module object",
        ));
    }
    let dict_bits = unsafe { module_dict_bits(module_ptr) };
    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "module dict missing",
        ));
    };
    if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "module dict missing",
        ));
    }
    Ok(dict_ptr)
}

pub(super) fn importlib_module_dict_get_optional_owned_bits(
    _py: &PyToken<'_>,
    module_bits: u64,
    attr_bits: u64,
) -> Result<Option<u64>, u64> {
    let dict_ptr = importlib_module_dict_ptr_for_state(_py, module_bits)?;
    let out = unsafe { dict_get_in_place(_py, dict_ptr, attr_bits) };
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let Some(bits) = out.filter(|bits| !obj_from_bits(*bits).is_none()) else {
        return Ok(None);
    };
    // `dict_get_in_place` returns a borrowed value. Importlib state repair
    // paths normalize all optional lookups to owned references so cleanup can
    // use a single explicit `dec_ref_bits` boundary without depending on where
    // the value came from.
    inc_ref_bits(_py, bits);
    Ok(Some(bits))
}

pub(super) fn importlib_stabilize_module_state_impl(
    _py: &PyToken<'_>,
    module_bits: u64,
    loader_bits: u64,
    origin_bits: u64,
    is_package: bool,
    module_package_bits: u64,
    package_root_bits: u64,
) -> Result<(), u64> {
    let mut spec_bits = MoltObject::none().bits();
    let mut spec_owned = false;
    let mut module_path_bits = MoltObject::none().bits();
    let mut module_path_owned = false;
    let out = (|| -> Result<(), u64> {
        let spec_name = intern_runtime_static_name(_py, b"__spec__");
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
            let dunder_path_name = intern_runtime_static_name(_py, b"__path__");
            if let Some(existing_path_bits) =
                importlib_module_dict_get_optional_owned_bits(_py, module_bits, dunder_path_name)?
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
                    runtime_static_name_slot(_py, b"__path__"),
                    b"__path__",
                    module_path_bits,
                )?;
            }

            if !obj_from_bits(spec_bits).is_none() {
                let locations_name = intern_static_name(
                    _py,
                    runtime_static_name_slot(_py, b"submodule_search_locations"),
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
                        runtime_static_name_slot(_py, b"submodule_search_locations"),
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
            let dunder_path_name = intern_runtime_static_name(_py, b"__path__");
            if let Some(module_path_attr_bits) =
                importlib_module_dict_get_optional_owned_bits(_py, module_bits, dunder_path_name)?
            {
                let should_delete = match obj_from_bits(module_path_attr_bits).as_ptr() {
                    Some(path_ptr) => unsafe { object_type_id(path_ptr) == TYPE_ID_OBJECT },
                    None => false,
                };
                if should_delete {
                    let module_dict_ptr = importlib_module_dict_ptr_for_state(_py, module_bits)?;
                    unsafe {
                        dict_del_in_place(_py, module_dict_ptr, dunder_path_name);
                    }
                    if exception_pending(_py) {
                        return Err(MoltObject::none().bits());
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
    out
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
    crate::with_gil_entry_nopanic!(_py, {
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

        match importlib_stabilize_module_state_impl(
            _py,
            module_bits,
            loader_bits,
            origin_bits,
            is_package,
            module_package_bits,
            package_root_bits,
        ) {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_spec_from_file_location_payload(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision("runpy.resolve_path", "fs.read", AuditArgs::None, allowed);
        if !allowed {
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
