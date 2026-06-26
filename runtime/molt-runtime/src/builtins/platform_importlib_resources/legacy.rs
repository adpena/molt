use super::*;

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_validate_resource_name(resource_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.resources.reader.resource_path_from_roots",
            "fs.read",
            AuditArgs::None,
            allowed,
        );
        if !allowed {
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
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.resources.reader.open_resource_bytes_from_roots",
            "fs.read",
            AuditArgs::None,
            allowed,
        );
        if !allowed {
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
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.resources.reader.is_resource_from_roots",
            "fs.read",
            AuditArgs::None,
            allowed,
        );
        if !allowed {
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
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.resources.reader.contents_from_roots",
            "fs.read",
            AuditArgs::None,
            allowed,
        );
        if !allowed {
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
pub extern "C" fn molt_importlib_resources_joinpath(traversable_bits: u64, child_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let joinpath_name = intern_runtime_static_name(_py, b"joinpath");
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
