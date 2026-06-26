use super::*;

pub(super) fn importlib_resources_module_name_from_bits(
    _py: &PyToken<'_>,
    module_bits: u64,
    fallback_bits: u64,
) -> Result<String, u64> {
    let module_name_name = intern_runtime_static_name(_py, b"__name__");
    if let Some(name_bits) = getattr_optional_bits(_py, module_bits, module_name_name)? {
        let out = string_obj_to_owned(obj_from_bits(name_bits));
        if !obj_from_bits(name_bits).is_none() {
            dec_ref_bits(_py, name_bits);
        }
        if let Some(name) = out
            && !name.is_empty()
        {
            return Ok(name);
        }
    }

    if !obj_from_bits(fallback_bits).is_none() {
        let Some(fallback) = string_obj_to_owned(obj_from_bits(fallback_bits)) else {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "fallback must be str or None",
            ));
        };
        if !fallback.is_empty() {
            return Ok(fallback);
        }
    }

    let spec_name = intern_runtime_static_name(_py, b"__spec__");
    if let Some(spec_bits) = getattr_optional_bits(_py, module_bits, spec_name)? {
        if let Some(spec_mod_name_bits) = getattr_optional_bits(_py, spec_bits, module_name_name)? {
            let out = string_obj_to_owned(obj_from_bits(spec_mod_name_bits));
            if !obj_from_bits(spec_mod_name_bits).is_none() {
                dec_ref_bits(_py, spec_mod_name_bits);
            }
            if !obj_from_bits(spec_bits).is_none() {
                dec_ref_bits(_py, spec_bits);
            }
            if let Some(name) = out
                && !name.is_empty()
            {
                return Ok(name);
            }
        } else if !obj_from_bits(spec_bits).is_none() {
            dec_ref_bits(_py, spec_bits);
        }
    }

    let package_name = intern_runtime_static_name(_py, b"__package__");
    if let Some(package_bits) = getattr_optional_bits(_py, module_bits, package_name)? {
        let out = string_obj_to_owned(obj_from_bits(package_bits));
        if !obj_from_bits(package_bits).is_none() {
            dec_ref_bits(_py, package_bits);
        }
        if let Some(name) = out
            && !name.is_empty()
        {
            return Ok(name);
        }
    }

    Err(raise_exception::<_>(_py, "ModuleNotFoundError", "unknown"))
}
pub(super) fn importlib_resources_loader_reader_from_bits(
    _py: &PyToken<'_>,
    module_bits: u64,
    module_name: &str,
) -> Result<Option<u64>, u64> {
    let loader_name = intern_runtime_static_name(_py, b"loader");
    let get_resource_reader_name = intern_runtime_static_name(_py, b"get_resource_reader");

    let try_loader = |loader_bits: u64| -> Result<Option<u64>, u64> {
        let Some(call_bits) =
            importlib_reader_lookup_callable(_py, loader_bits, get_resource_reader_name)?
        else {
            return Ok(None);
        };
        let module_name_bits = alloc_str_bits(_py, module_name)?;
        let reader_bits = unsafe { call_callable1(_py, call_bits, module_name_bits) };
        dec_ref_bits(_py, module_name_bits);
        dec_ref_bits(_py, call_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        if obj_from_bits(reader_bits).is_none() {
            return Ok(None);
        }
        Ok(Some(reader_bits))
    };

    let spec_name = intern_runtime_static_name(_py, b"__spec__");
    if let Some(spec_bits) = getattr_optional_bits(_py, module_bits, spec_name)? {
        let mut out: Option<u64> = None;
        if let Some(loader_bits) = getattr_optional_bits(_py, spec_bits, loader_name)? {
            out = try_loader(loader_bits)?;
            if !obj_from_bits(loader_bits).is_none() {
                dec_ref_bits(_py, loader_bits);
            }
        }
        if !obj_from_bits(spec_bits).is_none() {
            dec_ref_bits(_py, spec_bits);
        }
        if out.is_some() {
            return Ok(out);
        }
    }

    if let Some(loader_bits) = getattr_optional_bits(_py, module_bits, loader_name)? {
        let out = try_loader(loader_bits)?;
        if !obj_from_bits(loader_bits).is_none() {
            dec_ref_bits(_py, loader_bits);
        }
        return Ok(out);
    }

    Ok(None)
}
pub(super) fn importlib_reader_files_root_path(
    _py: &PyToken<'_>,
    reader_bits: u64,
) -> Result<Option<String>, u64> {
    let Some(value_bits) = importlib_reader_files_traversable_bits(_py, reader_bits)? else {
        return Ok(None);
    };
    let out = match path_from_bits(_py, value_bits) {
        Ok(path) => {
            let text = path.to_string_lossy().into_owned();
            if text.is_empty() { None } else { Some(text) }
        }
        Err(_) => {
            if exception_pending(_py) {
                clear_exception(_py);
            }
            None
        }
    };
    if !obj_from_bits(value_bits).is_none() {
        dec_ref_bits(_py, value_bits);
    }
    Ok(out)
}
pub(super) fn importlib_reader_join_parts_path(root: &str, parts: &[String]) -> String {
    let sep = bootstrap_path_sep();
    let mut path = root.to_string();
    for part in parts {
        if part.is_empty() {
            continue;
        }
        path = path_join_text(path, part, sep);
    }
    path
}
pub(super) fn importlib_reader_root_payload_for_parts(
    _py: &PyToken<'_>,
    reader_bits: u64,
    parts: &[String],
) -> Result<Option<(String, ImportlibResourcesPathPayload)>, u64> {
    let Some(root) = importlib_reader_files_root_path(_py, reader_bits)? else {
        return Ok(None);
    };
    let joined = importlib_reader_join_parts_path(&root, parts);
    let payload = importlib_resources_path_payload(&joined);
    Ok(Some((joined, payload)))
}
pub(super) fn importlib_resources_reader_roots_impl(
    _py: &PyToken<'_>,
    reader_bits: u64,
) -> Result<Vec<String>, u64> {
    let molt_roots_name = intern_runtime_static_name(_py, b"molt_roots");
    if let Some(call_bits) = importlib_reader_lookup_callable(_py, reader_bits, molt_roots_name)? {
        let values_bits = unsafe { call_callable0(_py, call_bits) };
        dec_ref_bits(_py, call_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        let roots = importlib_reader_collect_unique_paths(
            _py,
            values_bits,
            "invalid loader resource roots payload: list expected",
        )?;
        if !obj_from_bits(values_bits).is_none() {
            dec_ref_bits(_py, values_bits);
        }
        if !roots.is_empty() {
            return Ok(roots);
        }
    }

    if let Some(root) = importlib_reader_files_root_path(_py, reader_bits)? {
        return Ok(vec![root]);
    }

    Ok(Vec::new())
}
pub(super) fn importlib_resources_reader_contents_impl(
    _py: &PyToken<'_>,
    reader_bits: u64,
) -> Result<Vec<String>, u64> {
    let contents_name = intern_runtime_static_name(_py, b"contents");
    if let Some(call_bits) = importlib_reader_lookup_callable(_py, reader_bits, contents_name)? {
        let values_bits = unsafe { call_callable0(_py, call_bits) };
        dec_ref_bits(_py, call_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        if !obj_from_bits(values_bits).is_none() {
            let out = importlib_reader_collect_unique_strings(
                _py,
                values_bits,
                "invalid loader resource reader contents payload",
            )?;
            if !obj_from_bits(values_bits).is_none() {
                dec_ref_bits(_py, values_bits);
            }
            if !out.is_empty() {
                return Ok(out);
            }
        }
    }
    if let Some(root_bits) = importlib_traversable_bits_for_parts(_py, reader_bits, &[])? {
        let out = importlib_traversable_iterdir_names(_py, root_bits);
        if !obj_from_bits(root_bits).is_none() {
            dec_ref_bits(_py, root_bits);
        }
        return out;
    }
    if let Some((_root, payload)) = importlib_reader_root_payload_for_parts(_py, reader_bits, &[])?
        && payload.is_dir
    {
        return Ok(payload.entries);
    }
    Ok(Vec::new())
}
pub(super) fn importlib_resources_reader_resource_path_impl(
    _py: &PyToken<'_>,
    reader_bits: u64,
    name: &str,
) -> Result<Option<String>, u64> {
    let resource_path_name = intern_runtime_static_name(_py, b"resource_path");
    if let Some(call_bits) = importlib_reader_lookup_callable(_py, reader_bits, resource_path_name)?
    {
        let name_bits = alloc_str_bits(_py, name)?;
        let value_bits = unsafe { call_callable1(_py, call_bits, name_bits) };
        dec_ref_bits(_py, name_bits);
        dec_ref_bits(_py, call_bits);
        if exception_pending(_py) {
            if clear_pending_if_kind(
                _py,
                &["FileNotFoundError", "OSError", "NotImplementedError"],
            ) {
                return Ok(None);
            }
            return Err(MoltObject::none().bits());
        }
        let path = match path_from_bits(_py, value_bits) {
            Ok(path_buf) => path_buf.to_string_lossy().into_owned(),
            Err(_) => {
                if exception_pending(_py) {
                    clear_exception(_py);
                }
                if !obj_from_bits(value_bits).is_none() {
                    dec_ref_bits(_py, value_bits);
                }
                return Err(raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "invalid loader resource path payload",
                ));
            }
        };
        if !obj_from_bits(value_bits).is_none() {
            dec_ref_bits(_py, value_bits);
        }
        let payload = importlib_resources_path_payload(&path);
        if payload.is_file && !payload.is_archive_member {
            return Ok(Some(path));
        }
        return Ok(None);
    }

    let parts = importlib_resources_name_parts(name);
    if let Some(target_bits) = importlib_traversable_bits_for_parts(_py, reader_bits, &parts)? {
        let out = match path_from_bits(_py, target_bits) {
            Ok(path_buf) => {
                let path = path_buf.to_string_lossy().into_owned();
                let payload = importlib_resources_path_payload(&path);
                if payload.is_file && !payload.is_archive_member {
                    Some(path)
                } else {
                    None
                }
            }
            Err(_) => {
                if exception_pending(_py) {
                    clear_exception(_py);
                }
                None
            }
        };
        if !obj_from_bits(target_bits).is_none() {
            dec_ref_bits(_py, target_bits);
        }
        return Ok(out);
    }
    if let Some((joined, payload)) =
        importlib_reader_root_payload_for_parts(_py, reader_bits, &parts)?
        && payload.is_file
        && !payload.is_archive_member
    {
        return Ok(Some(joined));
    }
    Ok(None)
}
pub(super) fn importlib_resources_reader_child_names_impl(
    _py: &PyToken<'_>,
    reader_bits: u64,
    parts: &[String],
) -> Result<Vec<String>, u64> {
    if let Some(target_bits) = importlib_traversable_bits_for_parts(_py, reader_bits, parts)? {
        let names = importlib_traversable_iterdir_names(_py, target_bits);
        if !obj_from_bits(target_bits).is_none() {
            dec_ref_bits(_py, target_bits);
        }
        if let Ok(values) = names {
            return Ok(values);
        }
        return names;
    }
    if let Some((_joined, payload)) =
        importlib_reader_root_payload_for_parts(_py, reader_bits, parts)?
        && payload.is_dir
    {
        return Ok(payload.entries);
    }
    let entries = importlib_resources_reader_contents_impl(_py, reader_bits)?;
    let prefix = parts.join("/");
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for entry in entries {
        let remainder = if prefix.is_empty() {
            entry.as_str()
        } else if let Some(value) = entry.strip_prefix(&format!("{prefix}/")) {
            value
        } else {
            continue;
        };
        if remainder.is_empty() {
            continue;
        }
        let Some(child) = remainder.split('/').next() else {
            continue;
        };
        if child.is_empty() {
            continue;
        }
        let child_name = child.to_string();
        if seen.insert(child_name.clone()) {
            out.push(child_name);
        }
    }
    Ok(out)
}
pub(super) fn importlib_resources_reader_exists_impl(
    _py: &PyToken<'_>,
    reader_bits: u64,
    parts: &[String],
) -> Result<bool, u64> {
    if parts.is_empty() {
        return Ok(true);
    }
    if let Some(target_bits) = importlib_traversable_bits_for_parts(_py, reader_bits, parts)? {
        let out = importlib_traversable_exists(_py, target_bits);
        if !obj_from_bits(target_bits).is_none() {
            dec_ref_bits(_py, target_bits);
        }
        return out;
    }
    if let Some((_joined, payload)) =
        importlib_reader_root_payload_for_parts(_py, reader_bits, parts)?
    {
        return Ok(payload.exists);
    }
    let name = parts.join("/");
    if !name.is_empty() && importlib_resources_reader_is_resource_impl(_py, reader_bits, &name)? {
        return Ok(true);
    }
    let children = importlib_resources_reader_child_names_impl(_py, reader_bits, parts)?;
    Ok(!children.is_empty())
}
pub(super) fn importlib_resources_reader_is_dir_impl(
    _py: &PyToken<'_>,
    reader_bits: u64,
    parts: &[String],
) -> Result<bool, u64> {
    if parts.is_empty() {
        return Ok(true);
    }
    if let Some(target_bits) = importlib_traversable_bits_for_parts(_py, reader_bits, parts)? {
        let is_dir = importlib_traversable_is_dir(_py, target_bits)?;
        if is_dir {
            if !obj_from_bits(target_bits).is_none() {
                dec_ref_bits(_py, target_bits);
            }
            return Ok(true);
        }
        let is_file = importlib_traversable_is_file(_py, target_bits)?;
        if !obj_from_bits(target_bits).is_none() {
            dec_ref_bits(_py, target_bits);
        }
        return Ok(!is_file);
    }
    if let Some((_joined, payload)) =
        importlib_reader_root_payload_for_parts(_py, reader_bits, parts)?
    {
        return Ok(payload.is_dir);
    }
    let name = parts.join("/");
    if !name.is_empty() && importlib_resources_reader_is_resource_impl(_py, reader_bits, &name)? {
        return Ok(false);
    }
    let children = importlib_resources_reader_child_names_impl(_py, reader_bits, parts)?;
    Ok(!children.is_empty())
}
pub(super) fn importlib_resources_reader_is_resource_impl(
    _py: &PyToken<'_>,
    reader_bits: u64,
    name: &str,
) -> Result<bool, u64> {
    let is_resource_name = intern_runtime_static_name(_py, b"is_resource");
    if let Some(call_bits) = importlib_reader_lookup_callable(_py, reader_bits, is_resource_name)? {
        let name_bits = alloc_str_bits(_py, name)?;
        let value_bits = unsafe { call_callable1(_py, call_bits, name_bits) };
        dec_ref_bits(_py, name_bits);
        dec_ref_bits(_py, call_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        let true_bits = MoltObject::from_bool(true).bits();
        let false_bits = MoltObject::from_bool(false).bits();
        if value_bits == true_bits {
            if !obj_from_bits(value_bits).is_none() {
                dec_ref_bits(_py, value_bits);
            }
            return Ok(true);
        }
        if value_bits == false_bits {
            if !obj_from_bits(value_bits).is_none() {
                dec_ref_bits(_py, value_bits);
            }
            return Ok(false);
        }
        if !obj_from_bits(value_bits).is_none() {
            dec_ref_bits(_py, value_bits);
        }
        return Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            "invalid loader resource is_resource payload",
        ));
    }
    let parts = importlib_resources_name_parts(name);
    let Some(target_bits) = importlib_traversable_bits_for_parts(_py, reader_bits, &parts)? else {
        if let Some((_joined, payload)) =
            importlib_reader_root_payload_for_parts(_py, reader_bits, &parts)?
        {
            return Ok(payload.is_file);
        }
        return Ok(false);
    };
    let out = importlib_traversable_is_file(_py, target_bits);
    if !obj_from_bits(target_bits).is_none() {
        dec_ref_bits(_py, target_bits);
    }
    out
}
pub(super) fn importlib_resources_reader_open_resource_bytes_impl(
    _py: &PyToken<'_>,
    reader_bits: u64,
    name: &str,
) -> Result<Vec<u8>, u64> {
    let open_resource_name = intern_runtime_static_name(_py, b"open_resource");
    if let Some(call_bits) = importlib_reader_lookup_callable(_py, reader_bits, open_resource_name)?
    {
        let name_bits = alloc_str_bits(_py, name)?;
        let handle_bits = unsafe { call_callable1(_py, call_bits, name_bits) };
        dec_ref_bits(_py, name_bits);
        dec_ref_bits(_py, call_bits);
        if exception_pending(_py) {
            if clear_pending_if_kind(_py, &["NotImplementedError"]) {
                let parts = importlib_resources_name_parts(name);
                let Some(target_bits) =
                    importlib_traversable_bits_for_parts(_py, reader_bits, &parts)?
                else {
                    return Err(raise_exception::<_>(_py, "FileNotFoundError", name));
                };
                let out = importlib_traversable_open_bytes(_py, target_bits);
                if !obj_from_bits(target_bits).is_none() {
                    dec_ref_bits(_py, target_bits);
                }
                return out;
            }
            return Err(MoltObject::none().bits());
        }

        if let Some(bytes) = importlib_reader_collect_bytes(_py, handle_bits) {
            if !obj_from_bits(handle_bits).is_none() {
                dec_ref_bits(_py, handle_bits);
            }
            return Ok(bytes);
        }

        let read_name = intern_runtime_static_name(_py, b"read");
        let read_bits = match importlib_reader_lookup_callable(_py, handle_bits, read_name)? {
            Some(bits) => bits,
            None => {
                if !obj_from_bits(handle_bits).is_none() {
                    dec_ref_bits(_py, handle_bits);
                }
                return Err(raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "invalid loader open_resource payload",
                ));
            }
        };
        let payload_bits = unsafe { call_callable0(_py, read_bits) };
        dec_ref_bits(_py, read_bits);
        if exception_pending(_py) {
            if !obj_from_bits(handle_bits).is_none() {
                dec_ref_bits(_py, handle_bits);
            }
            return Err(MoltObject::none().bits());
        }

        let close_name = intern_runtime_static_name(_py, b"close");
        if let Some(close_bits) = importlib_reader_lookup_callable(_py, handle_bits, close_name)? {
            let _ = unsafe { call_callable0(_py, close_bits) };
            dec_ref_bits(_py, close_bits);
            if exception_pending(_py) {
                if !obj_from_bits(payload_bits).is_none() {
                    dec_ref_bits(_py, payload_bits);
                }
                if !obj_from_bits(handle_bits).is_none() {
                    dec_ref_bits(_py, handle_bits);
                }
                return Err(MoltObject::none().bits());
            }
        }
        if !obj_from_bits(handle_bits).is_none() {
            dec_ref_bits(_py, handle_bits);
        }

        let Some(bytes) = importlib_reader_collect_bytes(_py, payload_bits) else {
            if exception_pending(_py) {
                clear_exception(_py);
            }
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "invalid loader open_resource payload",
            ));
        };
        if !obj_from_bits(payload_bits).is_none() {
            dec_ref_bits(_py, payload_bits);
        }
        return Ok(bytes);
    }

    let parts = importlib_resources_name_parts(name);
    if let Some(target_bits) = importlib_traversable_bits_for_parts(_py, reader_bits, &parts)? {
        let out = importlib_traversable_open_bytes(_py, target_bits);
        if !obj_from_bits(target_bits).is_none() {
            dec_ref_bits(_py, target_bits);
        }
        return out;
    }
    if let Some((joined, payload)) =
        importlib_reader_root_payload_for_parts(_py, reader_bits, &parts)?
    {
        if !payload.is_file {
            return Err(raise_exception::<_>(_py, "FileNotFoundError", name));
        }
        return importlib_read_file_bytes(_py, &joined);
    }
    Err(raise_exception::<_>(_py, "FileNotFoundError", name))
}
