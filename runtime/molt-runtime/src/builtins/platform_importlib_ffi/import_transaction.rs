use super::*;
use crate::builtins::exceptions::ExceptionValue;
use crate::runtime_flags::{self, RuntimeFlag};

pub(super) fn importlib_find_in_path_payload(
    _py: &PyToken<'_>,
    fullname_bits: u64,
    search_paths_bits: u64,
    package_context: bool,
) -> u64 {
    let allowed = has_capability(_py, "fs.read");
    audit_capability_decision(
        "importlib.find.in_path_payload",
        "fs.read",
        AuditArgs::None,
        allowed,
    );
    if !allowed {
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
    let resolution =
        match importlib_resolve_spec_in_paths(_py, &fullname, &search_paths, package_context) {
            Ok(Some(resolution)) => resolution,
            Ok(None) => return MoltObject::none().bits(),
            Err(bits) => return bits,
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
                return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
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

pub(super) fn importlib_relative_level(name: &str) -> usize {
    name.as_bytes()
        .iter()
        .take_while(|&&byte| byte == b'.')
        .count()
}

pub(super) fn importlib_relative_base(package: &str, level: usize) -> Option<String> {
    if level == 0 {
        return Some(package.to_string());
    }
    let parts: Vec<&str> = package.split('.').collect();
    if level > parts.len() {
        return None;
    }
    Some(parts[..(parts.len() - level + 1)].join("."))
}

pub(super) fn importlib_resolve_name_arg(_py: &PyToken<'_>, name_bits: u64) -> Result<String, u64> {
    let name_obj = obj_from_bits(name_bits);
    let Some(name) = string_obj_to_owned(name_obj) else {
        return Err(raise_exception::<_>(
            _py,
            "AttributeError",
            &format!(
                "'{}' object has no attribute 'startswith'",
                type_name(_py, name_obj)
            ),
        ));
    };
    Ok(name)
}

pub(super) fn importlib_resolve_join(
    _py: &PyToken<'_>,
    name: &str,
    package: &str,
) -> Result<String, u64> {
    let level = importlib_relative_level(name);
    let Some(base) = importlib_relative_base(package, level) else {
        return Err(raise_exception::<_>(
            _py,
            "ImportError",
            "attempted relative import beyond top-level package",
        ));
    };
    let suffix = &name[level..];
    if suffix.is_empty() {
        return Ok(base);
    }
    if base.is_empty() {
        Ok(suffix.to_string())
    } else {
        Ok(format!("{base}.{suffix}"))
    }
}

pub(super) fn importlib_package_required_error(_py: &PyToken<'_>, name: &str) -> u64 {
    raise_exception::<_>(
        _py,
        "TypeError",
        &format!("the 'package' argument is required to perform a relative import for '{name}'"),
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resolve_name(name_bits: u64, package_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let name = match importlib_resolve_name_arg(_py, name_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if !name.starts_with('.') {
            return match alloc_str_bits(_py, &name) {
                Ok(bits) => bits,
                Err(err) => err,
            };
        }

        if !is_truthy(_py, obj_from_bits(package_bits)) {
            return raise_exception::<_>(
                _py,
                "ImportError",
                &format!("no package specified for '{name}' (required for relative module names)"),
            );
        }
        let package_obj = obj_from_bits(package_bits);
        let Some(package_name) = string_obj_to_owned(package_obj) else {
            return raise_exception::<_>(
                _py,
                "AttributeError",
                &format!(
                    "'{}' object has no attribute 'rsplit'",
                    type_name(_py, package_obj)
                ),
            );
        };

        match importlib_resolve_join(_py, &name, &package_name)
            .and_then(|resolved| alloc_str_bits(_py, &resolved))
        {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

pub(super) fn importlib_import_module_resolved_name(
    _py: &PyToken<'_>,
    name_bits: u64,
    package_bits: u64,
) -> Result<String, u64> {
    let name = importlib_resolve_name_arg(_py, name_bits)?;
    if name.is_empty() {
        return Err(raise_exception::<_>(_py, "ValueError", "Empty module name"));
    }
    if !name.starts_with('.') {
        return Ok(name);
    }
    if !is_truthy(_py, obj_from_bits(package_bits)) {
        return Err(importlib_package_required_error(_py, &name));
    }
    let Some(package_name) = string_obj_to_owned(obj_from_bits(package_bits)) else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "__package__ not set to a string",
        ));
    };
    if package_name.is_empty() {
        return Err(importlib_package_required_error(_py, &name));
    }
    importlib_resolve_join(_py, &name, &package_name)
}

pub(in super::super) enum ImportlibTransactionReturn<'a> {
    Leaf,
    Dunder {
        requested_name: &'a str,
        level: i64,
        fromlist_bits: u64,
    },
}

pub(in super::super) fn importlib_import_resolved_transaction(
    _py: &PyToken<'_>,
    resolved: &str,
    modules_ptr: *mut u8,
    return_plan: ImportlibTransactionReturn<'_>,
) -> Result<u64, u64> {
    let resolved_key_bits = alloc_str_bits(_py, resolved)?;
    let leaf_bits = importlib_import_resolved_module(
        _py,
        resolved,
        resolved_key_bits,
        modules_ptr,
        match &return_plan {
            ImportlibTransactionReturn::Leaf => ImportlibImportMode::Python,
            ImportlibTransactionReturn::Dunder { .. } => ImportlibImportMode::Builtin,
        },
    );
    dec_ref_bits(_py, resolved_key_bits);
    if exception_pending(_py) {
        if !obj_from_bits(leaf_bits).is_none() {
            dec_ref_bits(_py, leaf_bits);
        }
        return Err(MoltObject::none().bits());
    }

    let ImportlibTransactionReturn::Dunder {
        requested_name,
        level,
        fromlist_bits,
    } = return_plan
    else {
        return Ok(leaf_bits);
    };
    // __import__ selects the return module before package-only child preparation.
    // Ordinary modules leave __all__ entirely to indexed IMPORT_STAR; consuming
    // it here changes its protocol, exception precedence, and partial writes.
    let has_fromlist = is_truthy(_py, obj_from_bits(fromlist_bits));
    if exception_pending(_py) {
        dec_ref_bits(_py, leaf_bits);
        return Err(MoltObject::none().bits());
    }
    if !has_fromlist {
        return importlib_transaction_no_fromlist_value(
            _py,
            requested_name,
            level,
            resolved,
            modules_ptr,
            leaf_bits,
        );
    }
    if let Err(err) = importlib_prepare_package_fromlist(_py, resolved, leaf_bits, fromlist_bits) {
        dec_ref_bits(_py, leaf_bits);
        return Err(err);
    }
    Ok(leaf_bits)
}

pub(super) fn importlib_import_module_impl(
    _py: &PyToken<'_>,
    name_bits: u64,
    package_bits: u64,
) -> Result<u64, u64> {
    let resolved = importlib_import_module_resolved_name(_py, name_bits, package_bits)?;
    let modules_bits = importlib_runtime_modules_bits(_py)?;
    let Some(modules_ptr) = obj_from_bits(modules_bits).as_ptr() else {
        if !obj_from_bits(modules_bits).is_none() {
            dec_ref_bits(_py, modules_bits);
        }
        return Err(importlib_modules_runtime_error(_py));
    };

    // Cache precedence and provider/dependency provenance are resolved once by
    // the shared import attempt after the transaction's public-cache lookup.
    let out = importlib_import_resolved_transaction(
        _py,
        &resolved,
        modules_ptr,
        ImportlibTransactionReturn::Leaf,
    );
    if !obj_from_bits(modules_bits).is_none() {
        dec_ref_bits(_py, modules_bits);
    }
    out
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_import_module(name_bits: u64, package_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match importlib_import_module_impl(_py, name_bits, package_bits) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_import_optional(module_name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
    crate::with_gil_entry_nopanic!(_py, {
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
            // A required lookup: a miss raises the module's own AttributeError.
            let value_bits = crate::molt_get_attr_name(module_bits, attr_name_bits);
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
            return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
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
    crate::with_gil_entry_nopanic!(_py, {
        let fullname = match string_arg_from_bits(_py, fullname_bits, "fullname") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let call_bits = match importlib_required_callable(
            _py,
            bootstrap_bits,
            runtime_static_name_slot(_py, b"_load_module_shim"),
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

pub(super) fn importlib_dict_get_raw_key_bits(
    _py: &PyToken<'_>,
    dict_ptr: *mut u8,
    key_bits: u64,
) -> Result<Option<u64>, u64> {
    let value_bits = unsafe { dict_get_in_place(_py, dict_ptr, key_bits) };
    let value_bits = match value_bits {
        Ok(value) => value,
        Err(()) => {
            return Err(MoltObject::none().bits());
        }
    };
    Ok(value_bits)
}

pub(super) fn importlib_none_in_modules_error(
    _py: &PyToken<'_>,
    resolved: &str,
    name_bits: u64,
) -> u64 {
    crate::builtins::exceptions::raise_module_not_found(
        _py,
        format!("import of {resolved} halted; None in sys.modules").as_bytes(),
        name_bits,
    )
}

/// Only actual importer protocols may reach cold loading. CGetModule remains
/// a cache-only policy and cannot be supplied to this boundary.
#[derive(Clone, Copy)]
pub(crate) enum ImportlibImportMode {
    Python,
    Builtin,
}

impl ImportlibImportMode {
    fn cache_policy(self) -> crate::builtins::module_table::ModuleImportPolicy {
        use crate::builtins::module_table::ModuleImportPolicy;
        match self {
            Self::Python => ModuleImportPolicy::Python,
            Self::Builtin => ModuleImportPolicy::CImport,
        }
    }
}

pub(crate) fn importlib_import_resolved_attempt(
    _py: &PyToken<'_>,
    resolved: &str,
    resolved_key_bits: u64,
    modules_ptr: *mut u8,
    import_mode: ImportlibImportMode,
) -> Result<crate::builtins::modules::ModuleImportOutcome, u64> {
    use crate::builtins::modules::ModuleImportOutcome;
    let cached_bits = importlib_dict_get_raw_key_bits(_py, modules_ptr, resolved_key_bits)?;
    if let Some(cached_bits) = cached_bits {
        if obj_from_bits(cached_bits).is_none() {
            return Err(importlib_none_in_modules_error(
                _py,
                resolved,
                resolved_key_bits,
            ));
        }
        // Keep the actual caller dictionary and one owned observation through
        // metadata callbacks; only the selected policy may require a refresh.
        inc_ref_bits(_py, cached_bits);
        match crate::builtins::module_table::resolve_cached_module(
            _py,
            resolved,
            Some(resolved_key_bits),
            MoltObject::from_ptr(modules_ptr).bits(),
            cached_bits,
            import_mode.cache_policy(),
        ) {
            Ok(crate::builtins::module_table::CachedModuleOutcome::Return(bits)) => {
                return Ok(ModuleImportOutcome::Imported(bits));
            }
            Ok(crate::builtins::module_table::CachedModuleOutcome::Missing) => {
                unreachable!("ordinary import reloads")
            }
            Ok(crate::builtins::module_table::CachedModuleOutcome::Reload) => {
                // Re-enter existing importer/recursion admission once, rather
                // than spinning on user metadata that replaces its own entry.
                let Some(_depth) = crate::builtins::module_table::enter_import_recursion(_py)
                else {
                    return Err(MoltObject::none().bits());
                };
                return importlib_import_resolved_attempt(
                    _py,
                    resolved,
                    resolved_key_bits,
                    modules_ptr,
                    import_mode,
                );
            }
            Err(error) => return Err(error),
        }
    }

    match importlib_import_via_spec(_py, resolved, resolved_key_bits, modules_ptr, import_mode)? {
        Some(bits) => Ok(ModuleImportOutcome::Imported(bits)),
        None => Ok(ModuleImportOutcome::Missing {
            diagnostic_name: resolved.to_owned(),
        }),
    }
}

pub(super) fn importlib_import_resolved_module(
    py: &PyToken<'_>,
    resolved: &str,
    name: u64,
    modules: *mut u8,
    import_mode: ImportlibImportMode,
) -> u64 {
    match importlib_import_resolved_attempt(py, resolved, name, modules, import_mode) {
        Ok(crate::builtins::modules::ModuleImportOutcome::Imported(bits)) => bits,
        Ok(crate::builtins::modules::ModuleImportOutcome::Missing { .. }) => {
            crate::builtins::exceptions::raise_module_not_found(
                py,
                format!("No module named '{resolved}'").as_bytes(),
                name,
            )
        }
        Err(error) => error,
    }
}

pub(super) fn importlib_transaction_package_from_globals(
    _py: &PyToken<'_>,
    globals_bits: u64,
) -> Result<Option<String>, u64> {
    // The builtin's C default is NULL, even though its text signature displays
    // None. Preserve omission through named binding rather than accepting None.
    if globals_bits == crate::missing_bits(_py) {
        return Err(raise_exception::<_>(
            _py,
            "KeyError",
            "'__name__' not in globals",
        ));
    }
    let globals = crate::builtins::frames::globals_namespace_storage_ptr(_py, globals_bits)
        .map_err(|()| MoltObject::none().bits())?;
    let Some(globals_ptr) = globals else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "globals must be a dict",
        ));
    };

    let package_name = intern_runtime_static_name(_py, b"__package__");
    if let Some(package_bits) = unsafe { dict_get_in_place(_py, globals_ptr, package_name) }
        .map_err(|()| MoltObject::none().bits())?
        && !obj_from_bits(package_bits).is_none()
    {
        let Some(package) = string_obj_to_owned(obj_from_bits(package_bits)) else {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "package must be a string",
            ));
        };
        if !package.is_empty() {
            return Ok(Some(package));
        }
        return Ok(None);
    }

    let spec_name = intern_runtime_static_name(_py, b"__spec__");
    if let Some(spec_bits) = unsafe { dict_get_in_place(_py, globals_ptr, spec_name) }
        .map_err(|()| MoltObject::none().bits())?
        && !obj_from_bits(spec_bits).is_none()
    {
        let parent_name = intern_runtime_static_name(_py, b"parent");
        let parent_bits = crate::molt_object_getattribute(spec_bits, parent_name);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        let Some(parent) = string_obj_to_owned(obj_from_bits(parent_bits)) else {
            if !obj_from_bits(parent_bits).is_none() {
                dec_ref_bits(_py, parent_bits);
            }
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "__spec__.parent must be a string",
            ));
        };
        if !obj_from_bits(parent_bits).is_none() {
            dec_ref_bits(_py, parent_bits);
        }
        if !parent.is_empty() {
            return Ok(Some(parent));
        }
        return Ok(None);
    }

    let name_key = intern_runtime_static_name(_py, b"__name__");
    let Some(name_bits) = unsafe { dict_get_in_place(_py, globals_ptr, name_key) }
        .map_err(|()| MoltObject::none().bits())?
    else {
        return Err(raise_exception::<_>(
            _py,
            "KeyError",
            "'__name__' not in globals",
        ));
    };
    let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "__name__ must be a string",
        ));
    };
    if name.is_empty() {
        return Ok(None);
    }

    let path_name = intern_runtime_static_name(_py, b"__path__");
    let path_bits = unsafe { dict_get_in_place(_py, globals_ptr, path_name) };
    let path_bits = match path_bits {
        Ok(value) => value,
        Err(()) => {
            return Err(MoltObject::none().bits());
        }
    };
    if path_bits.is_some_and(|bits| !obj_from_bits(bits).is_none()) {
        return Ok(Some(name));
    }
    let Some((parent, _)) = name.rsplit_once('.') else {
        return Ok(None);
    };
    if parent.is_empty() {
        return Ok(None);
    }
    Ok(Some(parent.to_string()))
}

pub(super) fn importlib_transaction_resolved_name(
    _py: &PyToken<'_>,
    name: &str,
    globals_bits: u64,
    level: i64,
) -> Result<String, u64> {
    if level <= 0 {
        return Ok(name.to_string());
    }
    let Some(package) = importlib_transaction_package_from_globals(_py, globals_bits)? else {
        return Err(raise_exception::<_>(
            _py,
            "ImportError",
            "attempted relative import with no known parent package",
        ));
    };
    let level = level as usize;
    let Some(base) = importlib_relative_base(&package, level) else {
        return Err(raise_exception::<_>(
            _py,
            "ImportError",
            "attempted relative import beyond top-level package",
        ));
    };
    if name.is_empty() {
        return Ok(base);
    }
    if name.starts_with('.') {
        let missing_name = format!("{base}.");
        let key = alloc_str_bits(_py, &missing_name)?;
        let error = crate::builtins::exceptions::raise_module_not_found(
            _py,
            format!("No module named '{missing_name}'").as_bytes(),
            key,
        );
        dec_ref_bits(_py, key);
        return Err(error);
    }
    if base.is_empty() {
        Ok(name.to_string())
    } else {
        Ok(format!("{base}.{name}"))
    }
}

fn importlib_transaction_trace_enabled() -> bool {
    runtime_flags::enabled(RuntimeFlag::MOLT_TRACE_IMPORT_TRANSACTION)
}

fn importlib_transaction_fromlist_trace_display(_py: &PyToken<'_>, fromlist_bits: u64) -> String {
    let obj = obj_from_bits(fromlist_bits);
    let Some(ptr) = obj.as_ptr() else {
        return if obj.is_none() {
            "None".to_string()
        } else {
            format!("<{} bits=0x{fromlist_bits:x}>", type_name(_py, obj))
        };
    };
    let type_id = unsafe { object_type_id(ptr) };
    if type_id != TYPE_ID_TUPLE && type_id != TYPE_ID_LIST {
        return format!("<{} bits=0x{fromlist_bits:x}>", type_name(_py, obj));
    }

    let item_count = unsafe { crate::object::seq_access::locked_len(ptr) };
    let mut items = Vec::with_capacity(item_count);
    for index in 0..item_count {
        let Some(item) = (unsafe { crate::object::seq_access::pin_item(_py, ptr, index) }) else {
            break;
        };
        let item_bits = item.bits();
        let item_obj = obj_from_bits(item_bits);
        if let Some(text) = string_obj_to_owned(item_obj) {
            items.push(format!("{text:?}"));
        } else {
            items.push(format!(
                "<{} bits=0x{item_bits:x}>",
                type_name(_py, item_obj)
            ));
        }
    }
    format!("[{}]", items.join(", "))
}

fn trace_importlib_transaction(
    _py: &PyToken<'_>,
    name: &str,
    resolved: &str,
    globals_bits: u64,
    fromlist_bits: u64,
    level: i64,
) {
    if !importlib_transaction_trace_enabled() {
        return;
    }
    let fromlist = importlib_transaction_fromlist_trace_display(_py, fromlist_bits);
    eprintln!(
        "[molt import_transaction] name={name:?} level={level} resolved={resolved:?} fromlist={fromlist} globals_bits=0x{globals_bits:x} fromlist_bits=0x{fromlist_bits:x}"
    );
}

fn importlib_transaction_no_fromlist_value(
    _py: &PyToken<'_>,
    requested_name: &str,
    level: i64,
    resolved: &str,
    modules_ptr: *mut u8,
    leaf_bits: u64,
) -> Result<u64, u64> {
    // CPython selects the first component of the *request*, preserving the
    // resolved package prefix for relative imports. An empty or undotted
    // relative request returns the leaf without a second cache observation.
    let Some(dot) = requested_name.find('.') else {
        return Ok(leaf_bits);
    };
    let return_name = &resolved[..resolved.len() - requested_name.len() + dot];
    let return_key_bits = match alloc_str_bits(_py, return_name) {
        Ok(bits) => bits,
        Err(err) => {
            dec_ref_bits(_py, leaf_bits);
            return Err(err);
        }
    };
    let out = if level == 0 {
        // A false fromlist may mutate sys.modules. Absolute imports perform
        // the ordinary top-level import again, including finder/error policy.
        let bits = importlib_import_resolved_module(
            _py,
            return_name,
            return_key_bits,
            modules_ptr,
            ImportlibImportMode::Builtin,
        );
        if exception_pending(_py) {
            if !obj_from_bits(bits).is_none() {
                dec_ref_bits(_py, bits);
            }
            Err(MoltObject::none().bits())
        } else {
            Ok(bits)
        }
    } else {
        // The relative protocol performs only the raw dictionary lookup: a
        // present None is a result, while a removed entry raises KeyError.
        match importlib_dict_get_raw_key_bits(_py, modules_ptr, return_key_bits) {
            Ok(Some(bits)) => {
                inc_ref_bits(_py, bits);
                Ok(bits)
            }
            Ok(None) => Err(raise_exception::<_>(
                _py,
                "KeyError",
                &format!(
                    "{} not in sys.modules as expected",
                    crate::format_obj(_py, obj_from_bits(return_key_bits))
                ),
            )),
            Err(err) => Err(err),
        }
    };
    dec_ref_bits(_py, return_key_bits);
    // Both branches return a fresh owner even when cache entries alias the
    // imported leaf. Retire the incoming owner exactly once in every case.
    dec_ref_bits(_py, leaf_bits);
    out
}

pub(super) enum ImportlibTransactionStringItemsContext<'a> {
    FromList,
    ModuleAll { module_name: &'a str },
}

pub(super) fn importlib_transaction_string_items(
    _py: &PyToken<'_>,
    iterable_bits: u64,
    context: ImportlibTransactionStringItemsContext<'_>,
) -> Result<Vec<String>, u64> {
    let iter_bits = molt_iter(iterable_bits);
    let _iter_owner = obj_from_bits(iter_bits)
        .as_ptr()
        .map(crate::PtrDropGuard::new);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let mut out = Vec::new();
    loop {
        let pair_bits = molt_iter_next(iter_bits);
        let _pair_owner = obj_from_bits(pair_bits)
            .as_ptr()
            .map(crate::PtrDropGuard::new);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        let Some(pair_ptr) = maybe_ptr_from_bits(pair_bits) else {
            return Err(MoltObject::none().bits());
        };
        unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return Err(MoltObject::none().bits());
            }
        }
        if unsafe { crate::object::seq_access::locked_len(pair_ptr) } < 2 {
            return Err(MoltObject::none().bits());
        }
        let Some(done) = (unsafe { crate::object::seq_access::pin_item(_py, pair_ptr, 1) }) else {
            return Err(MoltObject::none().bits());
        };
        if is_truthy(_py, obj_from_bits(done.bits())) {
            break;
        }
        let Some(item) = (unsafe { crate::object::seq_access::pin_item(_py, pair_ptr, 0) }) else {
            return Err(MoltObject::none().bits());
        };
        let item_bits = item.bits();
        let item_obj = obj_from_bits(item_bits);
        let Some(text) = string_obj_to_owned(item_obj) else {
            let item_type = class_name_for_error(type_of_bits(_py, item_bits));
            let message = match &context {
                ImportlibTransactionStringItemsContext::FromList => {
                    format!("Item in ``from list'' must be str, not {item_type}")
                }
                ImportlibTransactionStringItemsContext::ModuleAll { module_name } => {
                    format!("Item in {module_name}.__all__ must be str, not {item_type}")
                }
            };
            return Err(raise_exception::<_>(_py, "TypeError", &message));
        };
        out.push(text);
    }
    Ok(out)
}

pub(super) fn importlib_transaction_child_name(resolved: &str, item: &str) -> String {
    if resolved == "molt.stdlib" {
        item.to_string()
    } else {
        format!("{resolved}.{item}")
    }
}

pub(super) fn importlib_transaction_module_all_items(
    _py: &PyToken<'_>,
    module_bits: u64,
) -> Result<Option<Vec<String>>, u64> {
    let Some(module_ptr) = obj_from_bits(module_bits).as_ptr() else {
        if exception_pending(_py) || obj_from_bits(module_bits).is_none() {
            return Ok(None);
        }
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "fromlist star expects module",
        ));
    };
    if unsafe { object_type_id(module_ptr) } != TYPE_ID_MODULE {
        if exception_pending(_py) {
            return Ok(None);
        }
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "fromlist star expects module",
        ));
    }
    let all_name_bits = intern_static_name(_py, &runtime_state(_py).interned.all_name, b"__all__");
    if all_name_bits == 0 {
        return Err(MoltObject::none().bits());
    }
    let all_bits = unsafe {
        crate::builtins::attr::attr_lookup_ptr_allow_missing(_py, module_ptr, all_name_bits)
    }
    .map_err(|()| MoltObject::none().bits())?;
    let Some(all_bits) = all_bits else {
        return Ok(None);
    };
    let module_name = unsafe { string_obj_to_owned(obj_from_bits(module_name_bits(module_ptr))) }
        .unwrap_or_default();
    let items = importlib_transaction_string_items(
        _py,
        all_bits,
        ImportlibTransactionStringItemsContext::ModuleAll {
            module_name: &module_name,
        },
    );
    dec_ref_bits(_py, all_bits);
    items.map(Some)
}

pub(super) fn importlib_transaction_prepare_fromlist_item(
    _py: &PyToken<'_>,
    resolved: &str,
    module_bits: u64,
    item: &str,
) -> Result<(), u64> {
    let attr_bits = alloc_str_bits(_py, item)?;
    let child_name = importlib_transaction_child_name(resolved, item);
    let child_name_bits = match alloc_str_bits(_py, &child_name) {
        Ok(bits) => bits,
        Err(err) => {
            dec_ref_bits(_py, attr_bits);
            return Err(err);
        }
    };
    let result = crate::builtins::modules::prepare_from_import_child(
        _py,
        module_bits,
        attr_bits,
        child_name_bits,
    );
    dec_ref_bits(_py, child_name_bits);
    dec_ref_bits(_py, attr_bits);
    result
}

pub(super) fn importlib_transaction_prepare_fromlist_star(
    _py: &PyToken<'_>,
    resolved: &str,
    module_bits: u64,
) -> Result<(), u64> {
    let Some(items) = importlib_transaction_module_all_items(_py, module_bits)? else {
        return Ok(());
    };
    for item in items {
        importlib_transaction_prepare_fromlist_item(_py, resolved, module_bits, &item)?;
    }
    Ok(())
}

pub(super) fn importlib_transaction_prepare_fromlist(
    _py: &PyToken<'_>,
    resolved: &str,
    module_bits: u64,
    fromlist_bits: u64,
) -> Result<(), u64> {
    for item in importlib_transaction_string_items(
        _py,
        fromlist_bits,
        ImportlibTransactionStringItemsContext::FromList,
    )? {
        if item == "*" {
            importlib_transaction_prepare_fromlist_star(_py, resolved, module_bits)?;
            continue;
        }
        importlib_transaction_prepare_fromlist_item(_py, resolved, module_bits, &item)?;
    }
    Ok(())
}

fn importlib_prepare_package_fromlist(
    py: &PyToken<'_>,
    resolved: &str,
    module_bits: u64,
    fromlist_bits: u64,
) -> Result<(), u64> {
    // Presence is the package boundary, including a falsey/dynamic __path__.
    // Only AttributeError is a miss. Both public imports and bootstrap bodies
    // use this owner before IMPORT_FROM observes any of the resulting names.
    let path_name = intern_runtime_static_name(py, b"__path__");
    let has_path = molt_has_attr_name(module_bits, path_name);
    if exception_pending(py) {
        return Err(MoltObject::none().bits());
    }
    if is_truthy(py, obj_from_bits(has_path)) {
        importlib_transaction_prepare_fromlist(py, resolved, module_bits, fromlist_bits)?;
    }
    Ok(())
}

/// Complete the fromlist stage of a compiler-owned bootstrap import. The raw
/// module load already happened; public __import__ is not entered recursively.
#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_prepare_fromlist(
    name_bits: u64,
    module_bits: u64,
    fromlist_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if exception_pending(py) {
            return MoltObject::none().bits();
        }
        let _name = ExceptionValue::pin(py, name_bits);
        let _module = ExceptionValue::pin(py, module_bits);
        let _fromlist = ExceptionValue::pin(py, fromlist_bits);
        let name = match string_arg_from_bits(py, name_bits, "module name") {
            Ok(name) => name,
            Err(error) => return error,
        };
        if !is_truthy(py, obj_from_bits(fromlist_bits)) || exception_pending(py) {
            return MoltObject::none().bits();
        }
        match importlib_prepare_package_fromlist(py, &name, module_bits, fromlist_bits) {
            Ok(()) => MoltObject::none().bits(),
            Err(error) => error,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_import_transaction(
    name_bits: u64,
    globals_bits: u64,
    _locals_bits: u64,
    fromlist_bits: u64,
    level_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let name = match string_arg_from_bits(_py, name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(level) = to_i64(obj_from_bits(level_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "level must be an integer");
        };
        if level < 0 {
            return raise_exception::<_>(_py, "ValueError", "level must be >= 0");
        }
        if name.is_empty() && level == 0 {
            return raise_exception::<_>(_py, "ValueError", "Empty module name");
        }
        let resolved = match importlib_transaction_resolved_name(_py, &name, globals_bits, level) {
            Ok(value) => value,
            Err(err) => return err,
        };
        trace_importlib_transaction(_py, &name, &resolved, globals_bits, fromlist_bits, level);
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
        let out = match importlib_import_resolved_transaction(
            _py,
            &resolved,
            modules_ptr,
            ImportlibTransactionReturn::Dunder {
                requested_name: &name,
                level,
                fromlist_bits,
            },
        ) {
            Ok(bits) => bits,
            Err(err) => {
                if !obj_from_bits(modules_bits).is_none() {
                    dec_ref_bits(_py, modules_bits);
                }
                return err;
            }
        };
        if !obj_from_bits(modules_bits).is_none() {
            dec_ref_bits(_py, modules_bits);
        }
        out
    })
}

#[cfg(test)]
#[path = "import_transaction/tests.rs"]
mod cached_module_tests;
