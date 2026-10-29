use super::*;
use crate::BuiltinExc;

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
            return raise_exception::<_>(
                _py,
                BuiltinExc::PermissionError,
                "missing fs.read capability",
            );
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
        let resolved = importlib_metadata_dist_paths(_py, &search_paths, module_file);
        match alloc_string_list_bits(_py, &resolved) {
            Some(bits) => bits,
            None => crate::builtins::exceptions::ExceptionSentinel::exception_sentinel(),
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
            return raise_exception::<_>(
                _py,
                BuiltinExc::PermissionError,
                "missing fs.read capability",
            );
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
        let payload = importlib_metadata_entry_points_payload(_py, &search_paths, module_file);
        match alloc_string_triplets_list_bits(_py, &payload) {
            Some(bits) => bits,
            None => crate::builtins::exceptions::ExceptionSentinel::exception_sentinel(),
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
            return raise_exception::<_>(
                _py,
                BuiltinExc::PermissionError,
                "missing fs.read capability",
            );
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
            _py,
            &search_paths,
            module_file,
            group.as_deref(),
            name.as_deref(),
            value.as_deref(),
        );
        match alloc_string_triplets_list_bits(_py, &payload) {
            Some(bits) => bits,
            None => crate::builtins::exceptions::ExceptionSentinel::exception_sentinel(),
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
            return Err(crate::builtins::exceptions::ExceptionSentinel::exception_sentinel());
        }
    };
    let entry_points_bits = match alloc_string_triplets_list_bits(_py, &payload.entry_points) {
        Some(bits) => bits,
        None => {
            dec_ref_bits(_py, path_bits);
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, version_bits);
            dec_ref_bits(_py, metadata_bits);
            return Err(crate::builtins::exceptions::ExceptionSentinel::exception_sentinel());
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
            return Err(crate::builtins::exceptions::ExceptionSentinel::exception_sentinel());
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
            return Err(crate::builtins::exceptions::ExceptionSentinel::exception_sentinel());
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
            return Err(crate::builtins::exceptions::ExceptionSentinel::exception_sentinel());
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
        Err(crate::builtins::exceptions::ExceptionSentinel::exception_sentinel())
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
            return raise_exception::<_>(
                _py,
                BuiltinExc::PermissionError,
                "missing fs.read capability",
            );
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
            return raise_exception::<_>(
                _py,
                BuiltinExc::PermissionError,
                "missing fs.read capability",
            );
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
        let payloads = importlib_metadata_distributions_payload(_py, &search_paths, module_file);
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
            crate::builtins::exceptions::ExceptionSentinel::exception_sentinel()
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
            return raise_exception::<_>(
                _py,
                BuiltinExc::PermissionError,
                "missing fs.read capability",
            );
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
                return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
            }
            tuple_bits_vec.push(MoltObject::from_ptr(tuple_ptr).bits());
        }
        let list_ptr = alloc_list(_py, tuple_bits_vec.as_slice());
        for bits in tuple_bits_vec {
            dec_ref_bits(_py, bits);
        }
        if list_ptr.is_null() {
            crate::builtins::exceptions::ExceptionSentinel::exception_sentinel()
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
            return raise_exception::<_>(
                _py,
                BuiltinExc::PermissionError,
                "missing fs.read capability",
            );
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
        let payload =
            importlib_metadata_packages_distributions_payload(_py, &search_paths, module_file);
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
                    return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
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
            crate::builtins::exceptions::ExceptionSentinel::exception_sentinel()
        } else {
            MoltObject::from_ptr(dict_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_module_from_spec(spec_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, { importlib_module_from_spec_impl(_py, spec_bits) })
}

/// The one spec-to-module attribute phase for creation and _exec reload.
/// Assignment catches AttributeError from its RHS or setter only, as
/// CPython _init_module_attrs does; other user exceptions remain selected.
pub(in super::super) fn importlib_init_module_attrs_impl(
    py: &PyToken<'_>,
    spec: u64,
    module: u64,
    override_existing: bool,
) -> Result<(), u64> {
    use crate::builtins::exceptions::ExceptionValue;
    let read = |object, name: &'static [u8]| -> Result<ExceptionValue<'_, '_>, u64> {
        let key = intern_runtime_static_name(py, name);
        if key == 0 {
            return Err(MoltObject::none().bits());
        }
        let bits = crate::builtins::attributes::get_attr_name(py, object, key)
            .map_err(|()| MoltObject::none().bits())?;
        Ok(ExceptionValue::adopt(py, bits))
    };
    let should_set = |name: &'static [u8]| -> Result<bool, u64> {
        if override_existing {
            return Ok(true);
        }
        let key = intern_runtime_static_name(py, name);
        if key == 0 {
            return Err(MoltObject::none().bits());
        }
        Ok(getattr_optional_bits(py, module, key)?
            .map(|bits| ExceptionValue::adopt(py, bits))
            .is_none_or(|value| obj_from_bits(value.bits()).is_none()))
    };
    let assign =
        |name: &'static [u8], get: &mut dyn FnMut() -> Result<u64, u64>| -> Result<(), u64> {
            let outcome = (|| {
                let value = get()?;
                let value = ExceptionValue::adopt(py, value);
                let key = intern_runtime_static_name(py, name);
                if key == 0 {
                    return Err(MoltObject::none().bits());
                }
                crate::builtins::attributes::mutate_attr_name(
                    py,
                    module,
                    key,
                    crate::builtins::attr::DescriptorMutation::Set(value.bits()),
                    molt_cpython_abi::hooks::AttributeMutation::Normal,
                )
                .map_err(|()| MoltObject::none().bits())?;
                if exception_pending(py) {
                    Err(MoltObject::none().bits())
                } else {
                    Ok(())
                }
            })();
            if outcome.is_err() && clear_pending_if_kind(py, &[crate::BuiltinExc::AttributeError]) {
                Ok(())
            } else {
                outcome
            }
        };
    if should_set(b"__name__")? {
        assign(b"__name__", &mut || {
            read(spec, b"name").map(ExceptionValue::into_bits)
        })?;
    }
    if should_set(b"__loader__")? {
        let mut loader = read(spec, b"loader")?;
        if obj_from_bits(loader.bits()).is_none() {
            let locations = read(spec, b"submodule_search_locations")?;
            if !obj_from_bits(locations.bits()).is_none() {
                // Reuse the class already owned by the machinery facade. Its
                // implicit namespace construction deliberately bypasses __init__
                // and retains the exact path object, as _init_module_attrs does.
                let dictionary = crate::builtins::module_table::module_dictionary(py)
                    .ok_or_else(|| MoltObject::none().bits())?;
                let machinery = ExceptionValue::adopt(
                    py,
                    importlib_module_support_bits(
                        py,
                        crate::ptr_from_bits(dictionary),
                        "importlib.machinery",
                    )?,
                );
                let class = read(machinery.bits(), b"NamespaceLoader")?;
                let new_name = ExceptionValue::adopt(
                    py,
                    attr_name_bits_from_bytes(py, b"__new__")
                        .ok_or_else(|| MoltObject::none().bits())?,
                );
                let new = ExceptionValue::adopt(
                    py,
                    crate::builtins::attributes::get_attr_name(py, class.bits(), new_name.bits())
                        .map_err(|()| MoltObject::none().bits())?,
                );
                let created = unsafe { call_callable1(py, new.bits(), class.bits()) };
                if exception_pending(py) {
                    return Err(MoltObject::none().bits());
                }
                loader = ExceptionValue::adopt(py, created);
                let path_name = ExceptionValue::adopt(
                    py,
                    attr_name_bits_from_bytes(py, b"_path")
                        .ok_or_else(|| MoltObject::none().bits())?,
                );
                crate::builtins::attributes::mutate_attr_name(
                    py,
                    loader.bits(),
                    path_name.bits(),
                    crate::builtins::attr::DescriptorMutation::Set(locations.bits()),
                    molt_cpython_abi::hooks::AttributeMutation::Normal,
                )
                .map_err(|()| MoltObject::none().bits())?;
                if exception_pending(py) {
                    return Err(MoltObject::none().bits());
                }
                let loader_name = intern_runtime_static_name(py, b"loader");
                if loader_name == 0 {
                    return Err(MoltObject::none().bits());
                }
                crate::builtins::attributes::mutate_attr_name(
                    py,
                    spec,
                    loader_name,
                    crate::builtins::attr::DescriptorMutation::Set(loader.bits()),
                    molt_cpython_abi::hooks::AttributeMutation::Normal,
                )
                .map_err(|()| MoltObject::none().bits())?;
                if exception_pending(py) {
                    return Err(MoltObject::none().bits());
                }
                // This compatibility assignment is outside CPython's caught
                // setter try: even AttributeError must propagate here.
                let file_name = intern_runtime_static_name(py, b"__file__");
                if file_name == 0 {
                    return Err(MoltObject::none().bits());
                }
                crate::builtins::attributes::mutate_attr_name(
                    py,
                    module,
                    file_name,
                    crate::builtins::attr::DescriptorMutation::Set(MoltObject::none().bits()),
                    molt_cpython_abi::hooks::AttributeMutation::Normal,
                )
                .map_err(|()| MoltObject::none().bits())?;
                if exception_pending(py) {
                    return Err(MoltObject::none().bits());
                }
            }
        }
        assign(b"__loader__", &mut || {
            inc_ref_bits(py, loader.bits());
            Ok(loader.bits())
        })?;
    }
    if should_set(b"__package__")? {
        assign(b"__package__", &mut || {
            read(spec, b"parent").map(ExceptionValue::into_bits)
        })?;
    }
    assign(b"__spec__", &mut || {
        inc_ref_bits(py, spec);
        Ok(spec)
    })?;
    if should_set(b"__path__")? {
        let locations = read(spec, b"submodule_search_locations")?;
        if !obj_from_bits(locations.bits()).is_none() {
            assign(b"__path__", &mut || {
                read(spec, b"submodule_search_locations").map(ExceptionValue::into_bits)
            })?;
        }
    }
    let has_location = read(spec, b"has_location")?;
    if crate::object::ops::truth_value(py, obj_from_bits(has_location.bits()))
        .map_err(|()| MoltObject::none().bits())?
    {
        if should_set(b"__file__")? {
            assign(b"__file__", &mut || {
                read(spec, b"origin").map(ExceptionValue::into_bits)
            })?;
        }
        if !crate::object::ops_sys::runtime_target_at_least(py, 3, 15) && should_set(b"__cached__")?
        {
            let cached = read(spec, b"cached")?;
            if !obj_from_bits(cached.bits()).is_none() {
                assign(b"__cached__", &mut || {
                    read(spec, b"cached").map(ExceptionValue::into_bits)
                })?;
            }
        }
    }
    Ok(())
}

pub(in super::super) fn importlib_module_from_spec_impl(_py: &PyToken<'_>, spec_bits: u64) -> u64 {
    let loader_name = intern_runtime_static_name(_py, b"loader");
    if loader_name == 0 {
        return MoltObject::none().bits();
    }
    let create_module_name = intern_runtime_static_name(_py, b"create_module");
    if create_module_name == 0 {
        return MoltObject::none().bits();
    }
    use crate::builtins::exceptions::ExceptionValue;
    let spec = ExceptionValue::pin(_py, spec_bits);
    let read_loader = || -> Result<ExceptionValue<'_, '_>, u64> {
        let bits = crate::builtins::attributes::get_attr_name(_py, spec.bits(), loader_name)
            .map_err(|()| MoltObject::none().bits())?;
        Ok(ExceptionValue::adopt(_py, bits))
    };
    let mut module_bits = MoltObject::none().bits();
    let out = (|| -> Result<u64, u64> {
        let present = {
            let loader = read_loader()?;
            getattr_optional_bits(_py, loader.bits(), create_module_name)?
                .map(|bits| ExceptionValue::adopt(_py, bits))
                .is_some()
        };
        if present {
            // hasattr and invocation are independent descriptor observations.
            // A present None/non-callable reaches the normal call error.
            let loader = read_loader()?;
            let create = ExceptionValue::adopt(
                _py,
                crate::builtins::attributes::get_attr_name(_py, loader.bits(), create_module_name)
                    .map_err(|()| MoltObject::none().bits())?,
            );
            drop(loader);
            let created = unsafe { call_callable1(_py, create.bits(), spec.bits()) };
            if exception_pending(_py) {
                molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(_py, created));
                return Err(MoltObject::none().bits());
            }
            module_bits = created;
        } else {
            let exec_name = intern_runtime_static_name(_py, b"exec_module");
            if exec_name == 0 {
                return Err(MoltObject::none().bits());
            }
            let present = {
                let loader = read_loader()?;
                getattr_optional_bits(_py, loader.bits(), exec_name)?
                    .map(|bits| ExceptionValue::adopt(_py, bits))
                    .is_some()
            };
            if present {
                return Err(raise_exception::<_>(
                    _py,
                    BuiltinExc::ImportError,
                    "loaders that define exec_module() must also define create_module()",
                ));
            }
        }

        if obj_from_bits(module_bits).is_none() {
            let name = crate::builtins::module_table::spec_name(_py, spec.bits())?;
            module_bits = crate::molt_module_new(name.bits());
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }
        }

        importlib_init_module_attrs_impl(_py, spec_bits, module_bits, false)?;
        Ok(module_bits)
    })();

    let result = match out {
        Ok(bits) => bits,
        Err(err) => {
            if !obj_from_bits(module_bits).is_none() {
                molt_cpython_abi::api::errors::with_preserved_error(|| {
                    dec_ref_bits(_py, module_bits)
                });
            }
            err
        }
    };
    molt_cpython_abi::api::errors::with_preserved_error(|| drop(spec));
    result
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
        use crate::builtins::exceptions::ExceptionValue;
        let origin_owner = if obj_from_bits(origin_bits).is_none() {
            let key = intern_runtime_static_name(_py, b"_ORIGIN");
            if key == 0 {
                return MoltObject::none().bits();
            }
            match getattr_optional_bits(_py, loader_bits, key) {
                Ok(value) => value.map(|bits| ExceptionValue::adopt(_py, bits)),
                Err(error) => return error,
            }
        } else {
            None
        };
        let origin_bits = origin_owner
            .as_ref()
            .map_or(origin_bits, ExceptionValue::bits);
        let origin_truth = match crate::object::ops::truth_value(_py, obj_from_bits(origin_bits)) {
            Ok(value) => value,
            Err(()) => return MoltObject::none().bits(),
        };
        if !origin_truth {
            let key = intern_runtime_static_name(_py, b"get_filename");
            if key == 0 {
                return MoltObject::none().bits();
            }
            let present = match getattr_optional_bits(_py, loader_bits, key) {
                Ok(value) => value.map(|bits| ExceptionValue::adopt(_py, bits)).is_some(),
                Err(error) => return error,
            };
            if present {
                let populate = obj_from_bits(is_package_bits).is_none();
                let search = if populate {
                    MoltObject::none().bits()
                } else {
                    match crate::object::ops::truth_value(_py, obj_from_bits(is_package_bits)) {
                        Ok(true) => {
                            let ptr = alloc_list(_py, &[]);
                            if ptr.is_null() {
                                return MoltObject::none().bits();
                            }
                            MoltObject::from_ptr(ptr).bits()
                        }
                        Ok(false) => MoltObject::none().bits(),
                        Err(()) => return MoltObject::none().bits(),
                    }
                };
                let search = ExceptionValue::adopt(_py, search);
                return importlib_spec_from_file_location_impl(
                    _py,
                    name_bits,
                    MoltObject::none().bits(),
                    loader_bits,
                    search.bits(),
                    machinery_bits,
                    populate,
                );
            }
        }
        let mut is_package_arg_bits = is_package_bits;
        let mut package_owner = None;
        if obj_from_bits(is_package_bits).is_none() {
            let key = intern_runtime_static_name(_py, b"is_package");
            if key == 0 {
                return MoltObject::none().bits();
            }
            let present = match getattr_optional_bits(_py, loader_bits, key) {
                Ok(value) => value.map(|bits| ExceptionValue::adopt(_py, bits)).is_some(),
                Err(error) => return error,
            };
            if present {
                let called = (|| -> Result<u64, u64> {
                    let call = ExceptionValue::adopt(
                        _py,
                        crate::builtins::attributes::get_attr_name(_py, loader_bits, key)
                            .map_err(|()| MoltObject::none().bits())?,
                    );
                    let value = unsafe { call_callable1(_py, call.bits(), name_bits) };
                    if exception_pending(_py) {
                        molt_cpython_abi::api::errors::with_preserved_error(|| {
                            dec_ref_bits(_py, value)
                        });
                        Err(MoltObject::none().bits())
                    } else {
                        Ok(value)
                    }
                })();
                match called {
                    Ok(value) => {
                        package_owner = Some(ExceptionValue::adopt(_py, value));
                        is_package_arg_bits = value;
                    }
                    Err(error) => {
                        // Only the invocation (including its descriptor read)
                        // catches ImportError. Constructor truth remains outside.
                        if !clear_pending_if_kind(_py, &[crate::BuiltinExc::ImportError]) {
                            return error;
                        }
                    }
                }
            } else {
                is_package_arg_bits = MoltObject::from_bool(false).bits();
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
                molt_cpython_abi::api::errors::with_preserved_error(|| {
                    dec_ref_bits(_py, module_spec_cls_bits);
                    drop(package_owner);
                });
                return err;
            }
        };
        molt_cpython_abi::api::errors::with_preserved_error(|| {
            dec_ref_bits(_py, module_spec_cls_bits);
            drop(package_owner);
        });
        if exception_pending(_py) {
            return MoltObject::none().bits();
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
        importlib_spec_from_file_location_impl(
            _py,
            name_bits,
            location_bits,
            loader_bits,
            submodule_search_locations_bits,
            machinery_bits,
            obj_from_bits(submodule_search_locations_bits).is_none(),
        )
    })
}

// `populate` distinguishes omitted search locations from an explicit None
// supplied by spec_from_loader(is_package=False), without a second factory.
fn importlib_spec_from_file_location_impl(
    _py: &PyToken<'_>,
    name_bits: u64,
    location_bits: u64,
    loader_bits: u64,
    submodule_search_locations_bits: u64,
    machinery_bits: u64,
    populate: bool,
) -> u64 {
    use crate::builtins::exceptions::ExceptionValue;
    let _name = match string_arg_from_bits(_py, name_bits, "name") {
        Ok(value) => value,
        Err(bits) => return bits,
    };
    let location_text_bits = if obj_from_bits(location_bits).is_none() {
        let mut location = match alloc_str_bits(_py, "<unknown>") {
            Ok(bits) => ExceptionValue::adopt(_py, bits),
            Err(error) => return error,
        };
        let key = intern_runtime_static_name(_py, b"get_filename");
        if key == 0 {
            return MoltObject::none().bits();
        }
        let present = match getattr_optional_bits(_py, loader_bits, key) {
            Ok(value) => value.map(|bits| ExceptionValue::adopt(_py, bits)).is_some(),
            Err(error) => return error,
        };
        if present {
            let called = (|| -> Result<u64, u64> {
                let method = ExceptionValue::adopt(
                    _py,
                    crate::builtins::attributes::get_attr_name(_py, loader_bits, key)
                        .map_err(|()| MoltObject::none().bits())?,
                );
                let value = unsafe { call_callable1(_py, method.bits(), name_bits) };
                if exception_pending(_py) {
                    molt_cpython_abi::api::errors::with_preserved_error(|| {
                        dec_ref_bits(_py, value)
                    });
                    Err(MoltObject::none().bits())
                } else {
                    Ok(value)
                }
            })();
            match called {
                Ok(bits) => location = ExceptionValue::adopt(_py, bits),
                Err(error) => {
                    if !clear_pending_if_kind(_py, &[crate::BuiltinExc::ImportError]) {
                        return error;
                    }
                }
            }
        }
        location.into_bits()
    } else {
        unsafe { call_callable1(_py, builtin_classes(_py).str, location_bits) }
    };
    if exception_pending(_py) {
        molt_cpython_abi::api::errors::with_preserved_error(|| {
            dec_ref_bits(_py, location_text_bits)
        });
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
        let module_spec_cls_bits = importlib_required_attribute(
            _py,
            machinery_bits,
            runtime_static_name_slot(_py, b"ModuleSpec"),
            b"ModuleSpec",
            "importlib.machinery",
        )?;
        let constructed = call_callable_positional(
            _py,
            module_spec_cls_bits,
            &[name_bits, loader_bits, path_bits, MoltObject::none().bits()],
        );
        molt_cpython_abi::api::errors::with_preserved_error(|| {
            dec_ref_bits(_py, module_spec_cls_bits)
        });
        spec_bits = constructed?;
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        // File provenance is admitted before loader/package callbacks.
        importlib_set_attr(
            _py,
            spec_bits,
            runtime_static_name_slot(_py, b"has_location"),
            b"has_location",
            MoltObject::from_bool(true).bits(),
        )?;
        if obj_from_bits(loader_bits).is_none() {
            let loader_cls_bits = importlib_required_attribute(
                _py,
                machinery_bits,
                runtime_static_name_slot(_py, b"SourceFileLoader"),
                b"SourceFileLoader",
                "importlib.machinery",
            )?;
            let loaded = call_callable_positional(_py, loader_cls_bits, &[name_bits, path_bits]);
            molt_cpython_abi::api::errors::with_preserved_error(|| {
                dec_ref_bits(_py, loader_cls_bits)
            });
            effective_loader_bits = loaded?;
            effective_loader_owned = true;
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }
            importlib_set_attr(
                _py,
                spec_bits,
                runtime_static_name_slot(_py, b"loader"),
                b"loader",
                effective_loader_bits,
            )?;
        }

        if !obj_from_bits(submodule_search_locations_bits).is_none() {
            locations_bits = submodule_search_locations_bits;
            inc_ref_bits(_py, locations_bits);
            locations_owned = true;
        } else if populate {
            let key = intern_runtime_static_name(_py, b"is_package");
            if key == 0 {
                return Err(MoltObject::none().bits());
            }
            let present = getattr_optional_bits(_py, effective_loader_bits, key)?
                .map(|bits| ExceptionValue::adopt(_py, bits))
                .is_some();
            if present {
                let called = (|| -> Result<u64, u64> {
                    let method = ExceptionValue::adopt(
                        _py,
                        crate::builtins::attributes::get_attr_name(_py, effective_loader_bits, key)
                            .map_err(|()| MoltObject::none().bits())?,
                    );
                    let value = unsafe { call_callable1(_py, method.bits(), name_bits) };
                    if exception_pending(_py) {
                        molt_cpython_abi::api::errors::with_preserved_error(|| {
                            dec_ref_bits(_py, value)
                        });
                        Err(MoltObject::none().bits())
                    } else {
                        Ok(value)
                    }
                })();
                match called {
                    Ok(value) => {
                        let value = ExceptionValue::adopt(_py, value);
                        if crate::object::ops::truth_value(_py, obj_from_bits(value.bits()))
                            .map_err(|()| MoltObject::none().bits())?
                        {
                            let ptr = alloc_list(_py, &[]);
                            if ptr.is_null() {
                                return Err(MoltObject::none().bits());
                            }
                            locations_bits = MoltObject::from_ptr(ptr).bits();
                            locations_owned = true;
                        }
                    }
                    Err(error) => {
                        if !clear_pending_if_kind(_py, &[crate::BuiltinExc::ImportError]) {
                            return Err(error);
                        }
                    }
                }
            }
        }

        if !populate || !obj_from_bits(locations_bits).is_none() {
            importlib_set_attr(
                _py,
                spec_bits,
                runtime_static_name_slot(_py, b"submodule_search_locations"),
                b"submodule_search_locations",
                locations_bits,
            )?;
        }
        if let Some(ptr) = obj_from_bits(locations_bits).as_ptr()
            && unsafe { object_type_id(ptr) } == TYPE_ID_LIST
            && unsafe { crate::builtins::containers::list_len(ptr) } == 0
            && !path.is_empty()
        {
            let root = path_dirname_text(&path, bootstrap_path_sep());
            let root = ExceptionValue::adopt(_py, alloc_str_bits(_py, &root)?);
            let result = crate::molt_list_append(locations_bits, root.bits());
            if exception_pending(_py) {
                return Err(result);
            }
        }
        Ok(spec_bits)
    })();

    molt_cpython_abi::api::errors::with_preserved_error(|| {
        if !obj_from_bits(path_bits).is_none() {
            dec_ref_bits(_py, path_bits);
        }
        if locations_owned && !obj_from_bits(locations_bits).is_none() {
            dec_ref_bits(_py, locations_bits);
        }
        if effective_loader_owned && !obj_from_bits(effective_loader_bits).is_none() {
            dec_ref_bits(_py, effective_loader_bits);
        }
    });
    match out {
        Ok(bits) => bits,
        Err(err) => {
            if !obj_from_bits(spec_bits).is_none() {
                molt_cpython_abi::api::errors::with_preserved_error(|| {
                    dec_ref_bits(_py, spec_bits)
                });
            }
            err
        }
    }
}

pub(super) fn importlib_module_dict_ptr_for_state(
    _py: &PyToken<'_>,
    module_bits: u64,
) -> Result<*mut u8, u64> {
    let Some(module_ptr) = obj_from_bits(module_bits).as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            BuiltinExc::TypeError,
            "module state expects module object",
        ));
    };
    if unsafe { object_type_id(module_ptr) } != TYPE_ID_MODULE {
        return Err(raise_exception::<_>(
            _py,
            BuiltinExc::TypeError,
            "module state expects module object",
        ));
    }
    let dict_bits = unsafe { module_dict_bits(module_ptr) };
    let Some(dict_ptr) = obj_from_bits(dict_bits).as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            BuiltinExc::TypeError,
            "module dict missing",
        ));
    };
    if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
        return Err(raise_exception::<_>(
            _py,
            BuiltinExc::TypeError,
            "module dict missing",
        ));
    }
    Ok(dict_ptr)
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
