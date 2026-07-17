//! CPython-compatible runpy policy over compiler-emitted module execution.
//!
//! Source parsing and import dispatch belong to the compiler/import runtime,
//! not to runpy.  This module resolves the requested compiled module, owns the
//! result namespace and argv policy, and delegates every body execution to
//! `modules::execution`.

use super::*;

fn runpy_normalize_candidate(path: PathBuf) -> String {
    std::fs::canonicalize(&path)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

fn vfs_stat(path: &std::path::Path) -> Option<bool> {
    let path_text = path.to_string_lossy();
    if let Some(state) = crate::runtime_state_for_gil()
        && let Some(vfs) = state.get_vfs()
        && let Some((_prefix, backend, rel)) = vfs.resolve(&path_text)
    {
        return backend.stat(&rel).ok().map(|stat| stat.is_dir);
    }
    std::fs::metadata(path)
        .ok()
        .map(|metadata| metadata.is_dir())
}

fn vfs_is_file(path: &std::path::Path) -> bool {
    vfs_stat(path).is_some_and(|is_dir| !is_dir)
}

fn is_module_name_component(value: &str) -> bool {
    !value.is_empty()
        && !value.contains('.')
        && !value.contains('/')
        && !value.contains('\\')
        && !value.contains('\0')
}

fn module_name_from_relative_path(relative: &std::path::Path) -> Option<String> {
    let mut parts = relative
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>();
    let filename = parts.pop()?;
    let stem = filename.strip_suffix(".py")?;
    if stem != "__init__" {
        parts.push(stem.to_string());
    }
    if parts.is_empty() || !parts.iter().all(|part| is_module_name_component(part)) {
        return None;
    }
    Some(parts.join("."))
}

fn runpy_module_name_for_path(path: &std::path::Path, sys_path: &[String]) -> Option<String> {
    let canonical = std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf());
    for base in sys_path {
        let base_path = if base.is_empty() {
            PathBuf::from(".")
        } else {
            PathBuf::from(base)
        };
        let base_canonical = std::fs::canonicalize(&base_path).unwrap_or(base_path);
        if let Ok(relative) = canonical.strip_prefix(&base_canonical)
            && let Some(name) = module_name_from_relative_path(relative)
        {
            return Some(name);
        }
    }
    None
}

struct RunpyModuleTarget {
    import_name: String,
    origin: String,
}

fn runpy_catalog_target(mod_name: &str) -> Option<Result<RunpyModuleTarget, String>> {
    let is_package = crate::builtins::module_table::module_catalog_is_package(mod_name)?;
    if !is_package {
        return (crate::builtins::module_table::module_execution_target_has_body(mod_name)
            == Some(true))
        .then(|| {
            Ok(RunpyModuleTarget {
                import_name: mod_name.to_string(),
                origin: crate::builtins::module_table::module_catalog_origin(mod_name)
                    .filter(|origin| !origin.is_empty())
                    .unwrap_or("<compiled>")
                    .to_string(),
            })
        });
    }
    let main_name = format!("{mod_name}.__main__");
    if crate::builtins::module_table::module_execution_target_has_body(&main_name) == Some(true) {
        return Some(Ok(RunpyModuleTarget {
            origin: crate::builtins::module_table::module_catalog_origin(&main_name)
                .filter(|origin| !origin.is_empty())
                .unwrap_or("<compiled>")
                .to_string(),
            import_name: main_name,
        }));
    }
    Some(Err(format!(
        "No module named {main_name:?}; {mod_name:?} is a package and cannot be directly executed"
    )))
}

fn runpy_optional_attr(
    _py: &PyToken<'_>,
    target_bits: u64,
    name_bits: u64,
) -> Result<Option<u64>, u64> {
    let missing = missing_bits(_py);
    let value_bits = molt_getattr_builtin(target_bits, name_bits, missing);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if is_missing_bits(_py, value_bits) {
        return Ok(None);
    }
    Ok(Some(value_bits))
}

fn runpy_find_spec(_py: &PyToken<'_>, module_name: &str) -> Result<Option<u64>, u64> {
    let util_name_ptr = alloc_string(_py, b"importlib.util");
    if util_name_ptr.is_null() {
        return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
    }
    let util_name_bits = MoltObject::from_ptr(util_name_ptr).bits();
    let util_bits = molt_module_import_inner(util_name_bits);
    dec_ref_bits(_py, util_name_bits);
    if exception_pending(_py) {
        if !obj_from_bits(util_bits).is_none() {
            dec_ref_bits(_py, util_bits);
        }
        return Err(MoltObject::none().bits());
    }
    let find_spec_name = crate::intern_runtime_static_name(_py, b"find_spec");
    let find_spec_bits = match runpy_optional_attr(_py, util_bits, find_spec_name) {
        Ok(Some(bits)) => bits,
        Ok(None) => {
            dec_ref_bits(_py, util_bits);
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "importlib.util.find_spec is unavailable",
            ));
        }
        Err(bits) => {
            dec_ref_bits(_py, util_bits);
            return Err(bits);
        }
    };
    dec_ref_bits(_py, util_bits);
    let name_ptr = alloc_string(_py, module_name.as_bytes());
    if name_ptr.is_null() {
        dec_ref_bits(_py, find_spec_bits);
        return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
    }
    let name_bits = MoltObject::from_ptr(name_ptr).bits();
    let spec_bits = unsafe { call_callable1(_py, find_spec_bits, name_bits) };
    dec_ref_bits(_py, name_bits);
    dec_ref_bits(_py, find_spec_bits);
    if exception_pending(_py) {
        if !obj_from_bits(spec_bits).is_none() {
            dec_ref_bits(_py, spec_bits);
        }
        return Err(MoltObject::none().bits());
    }
    if obj_from_bits(spec_bits).is_none() {
        return Ok(None);
    }
    Ok(Some(spec_bits))
}

fn runpy_spec_is_package(_py: &PyToken<'_>, spec_bits: u64) -> Result<bool, u64> {
    let name = crate::intern_runtime_static_name(_py, b"submodule_search_locations");
    let Some(value_bits) = runpy_optional_attr(_py, spec_bits, name)? else {
        return Ok(false);
    };
    let is_package = !obj_from_bits(value_bits).is_none();
    if !obj_from_bits(value_bits).is_none() {
        dec_ref_bits(_py, value_bits);
    }
    Ok(is_package)
}

fn runpy_spec_origin(_py: &PyToken<'_>, spec_bits: u64, fallback: &str) -> Result<String, u64> {
    let name = crate::intern_runtime_static_name(_py, b"origin");
    let Some(value_bits) = runpy_optional_attr(_py, spec_bits, name)? else {
        return Ok(fallback.to_string());
    };
    let origin =
        string_obj_to_owned(obj_from_bits(value_bits)).unwrap_or_else(|| fallback.to_string());
    if !obj_from_bits(value_bits).is_none() {
        dec_ref_bits(_py, value_bits);
    }
    Ok(origin)
}

fn runpy_resolve_module(_py: &PyToken<'_>, mod_name: &str) -> Result<RunpyModuleTarget, u64> {
    if mod_name.starts_with('.') {
        return Err(raise_exception::<_>(
            _py,
            "ImportError",
            "Relative module names not supported",
        ));
    }
    if mod_name.is_empty() || !mod_name.split('.').all(is_module_name_component) {
        return Err(raise_exception::<_>(
            _py,
            "ImportError",
            &format!("No module named {mod_name}"),
        ));
    }
    let Some(mut spec_bits) = runpy_find_spec(_py, mod_name)? else {
        if let Some(catalog) = runpy_catalog_target(mod_name) {
            return catalog.map_err(|message| raise_exception::<_>(_py, "ImportError", &message));
        }
        return Err(raise_exception::<_>(
            _py,
            "ImportError",
            &format!("No module named {mod_name}"),
        ));
    };
    let mut import_name = mod_name.to_string();
    let is_package = match runpy_spec_is_package(_py, spec_bits) {
        Ok(value) => value,
        Err(bits) => {
            dec_ref_bits(_py, spec_bits);
            return Err(bits);
        }
    };
    if is_package {
        dec_ref_bits(_py, spec_bits);
        import_name = format!("{mod_name}.__main__");
        let Some(main_spec_bits) = runpy_find_spec(_py, &import_name)? else {
            if crate::builtins::module_table::module_execution_target_has_body(&import_name)
                == Some(true)
            {
                return Ok(RunpyModuleTarget {
                    origin: crate::builtins::module_table::module_catalog_origin(&import_name)
                        .filter(|origin| !origin.is_empty())
                        .unwrap_or("<compiled>")
                        .to_string(),
                    import_name,
                });
            }
            return Err(raise_exception::<_>(
                _py,
                "ImportError",
                &format!(
                    "No module named {import_name:?}; {mod_name:?} is a package and cannot be directly executed"
                ),
            ));
        };
        spec_bits = main_spec_bits;
    }
    let origin = match runpy_spec_origin(_py, spec_bits, &import_name) {
        Ok(value) => value,
        Err(bits) => {
            dec_ref_bits(_py, spec_bits);
            return Err(bits);
        }
    };
    dec_ref_bits(_py, spec_bits);
    Ok(RunpyModuleTarget {
        import_name,
        origin,
    })
}

enum RunpyPathResolutionError {
    MissingPath(String),
    DirectoryWithoutMain(String),
    OutsideCompiledClosure(String),
}

struct RunpyPathTarget {
    source_path: String,
    argv0_path: String,
    import_name: String,
    import_container: bool,
}

fn runpy_resolve_path(
    path: &str,
    sys_path: &[String],
) -> Result<RunpyPathTarget, RunpyPathResolutionError> {
    let raw = PathBuf::from(path);
    let normalized = runpy_normalize_candidate(raw.clone());
    if let Some(import_name) =
        crate::builtins::module_table::module_catalog_name_by_origin(&normalized)
    {
        return Ok(RunpyPathTarget {
            source_path: normalized,
            argv0_path: path.to_string(),
            import_name: import_name.to_string(),
            import_container: false,
        });
    }
    let main = raw.join("__main__.py");
    let normalized_main = runpy_normalize_candidate(main);
    if let Some(import_name) =
        crate::builtins::module_table::module_catalog_name_by_origin(&normalized_main)
    {
        return Ok(RunpyPathTarget {
            source_path: normalized_main,
            argv0_path: path.to_string(),
            import_name: import_name.to_string(),
            import_container: true,
        });
    }
    let is_dir =
        vfs_stat(&raw).ok_or_else(|| RunpyPathResolutionError::MissingPath(path.to_string()))?;
    let executable = if is_dir {
        let main = raw.join("__main__.py");
        if !vfs_is_file(&main) {
            return Err(RunpyPathResolutionError::DirectoryWithoutMain(
                path.to_string(),
            ));
        }
        main
    } else {
        raw
    };
    let normalized = runpy_normalize_candidate(executable.clone());
    let module_name = runpy_module_name_for_path(&executable, sys_path)
        .ok_or_else(|| RunpyPathResolutionError::OutsideCompiledClosure(normalized.clone()))?;
    Ok(RunpyPathTarget {
        source_path: executable.to_string_lossy().into_owned(),
        argv0_path: path.to_string(),
        import_name: module_name,
        import_container: is_dir,
    })
}

unsafe fn runpy_namespace_from_module(_py: &PyToken<'_>, module_bits: u64) -> Result<u64, u64> {
    unsafe {
        let source_dict_ptr = module_dict_ptr(_py, module_bits)?;
        let out_ptr = alloc_dict_with_pairs(_py, &[]);
        if out_ptr.is_null() {
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        copy_dict_entries(_py, source_dict_ptr, out_ptr);
        if exception_pending(_py) {
            let out_bits = MoltObject::from_ptr(out_ptr).bits();
            dec_ref_bits(_py, out_bits);
            return Err(MoltObject::none().bits());
        }
        Ok(MoltObject::from_ptr(out_ptr).bits())
    }
}

fn init_globals_bits(init_globals_bits: u64) -> Result<Option<u64>, &'static str> {
    let init_obj = obj_from_bits(init_globals_bits);
    if init_obj.is_none() {
        return Ok(None);
    }
    match init_obj.as_ptr() {
        Some(ptr) if unsafe { object_type_id(ptr) == TYPE_ID_DICT } => Ok(Some(init_globals_bits)),
        _ => Err("init_globals must be dict or None"),
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_runpy_run_module(
    mod_name_bits: u64,
    run_name_bits: u64,
    init_globals_bits_value: u64,
    alter_sys_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mod_name = match string_obj_to_owned(obj_from_bits(mod_name_bits)) {
            Some(value) => value,
            None => return raise_exception::<_>(_py, "TypeError", "mod_name must be str"),
        };
        let requested_run_name = if obj_from_bits(run_name_bits).is_none() {
            None
        } else {
            match string_obj_to_owned(obj_from_bits(run_name_bits)) {
                Some(value) => Some(value),
                None => return raise_exception::<_>(_py, "TypeError", "run_name must be str"),
            }
        };
        let initial = match init_globals_bits(init_globals_bits_value) {
            Ok(value) => value,
            Err(message) => return raise_exception::<_>(_py, "TypeError", message),
        };
        let alter_sys = is_truthy(_py, obj_from_bits(alter_sys_bits));
        let target = match runpy_resolve_module(_py, &mod_name) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let RunpyModuleTarget {
            import_name,
            origin,
        } = target;
        let run_name = requested_run_name.as_deref().unwrap_or(&import_name);
        let metadata = if alter_sys {
            ExecutionMetadata::Module {
                argv0: Some(origin),
            }
        } else {
            ExecutionMetadata::Module { argv0: None }
        };
        let module =
            execute_compiled_module(_py, &import_name, run_name, initial, alter_sys, metadata);
        let module_bits = match module {
            Ok(bits) => bits,
            Err(err) => return err.into_import_error(_py, &import_name),
        };
        let namespace = unsafe { runpy_namespace_from_module(_py, module_bits) };
        dec_ref_bits(_py, module_bits);
        match namespace {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_runpy_run_path(
    path_bits: u64,
    run_name_bits: u64,
    init_globals_bits_value: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision("runpy.run_path", "fs.read", AuditArgs::None, allowed);
        if !allowed {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match string_obj_to_owned(obj_from_bits(path_bits)) {
            Some(value) => value,
            None => return raise_exception::<_>(_py, "TypeError", "path must be str"),
        };
        let run_name = if obj_from_bits(run_name_bits).is_none() {
            "<run_path>".to_string()
        } else {
            match string_obj_to_owned(obj_from_bits(run_name_bits)) {
                Some(value) => value,
                None => return raise_exception::<_>(_py, "TypeError", "run_name must be str"),
            }
        };
        let initial = match init_globals_bits(init_globals_bits_value) {
            Ok(value) => value,
            Err(message) => return raise_exception::<_>(_py, "TypeError", message),
        };
        let sys_path = match unsafe { execution_sys_path_entries(_py) } {
            Ok(entries) => entries,
            Err(bits) => return bits,
        };
        let target = match runpy_resolve_path(&path, &sys_path) {
            Ok(value) => value,
            Err(RunpyPathResolutionError::MissingPath(missing)) => {
                return raise_exception::<_>(
                    _py,
                    "FileNotFoundError",
                    &format!("No such file or directory: {missing:?}"),
                );
            }
            Err(RunpyPathResolutionError::DirectoryWithoutMain(directory)) => {
                return raise_exception::<_>(
                    _py,
                    "ImportError",
                    &format!("can't find '__main__' module in {directory:?}"),
                );
            }
            Err(RunpyPathResolutionError::OutsideCompiledClosure(source)) => {
                return raise_exception::<_>(
                    _py,
                    "ImportError",
                    &format!(
                        "run_path target {source:?} is not part of this binary's compiled module closure"
                    ),
                );
            }
        };
        let RunpyPathTarget {
            source_path,
            argv0_path,
            import_name,
            import_container,
        } = target;
        let metadata = if import_container {
            ExecutionMetadata::ImportContainer(argv0_path)
        } else {
            ExecutionMetadata::ScriptFile(source_path)
        };
        let module = execute_compiled_module(_py, &import_name, &run_name, initial, true, metadata);
        let module_bits = match module {
            Ok(bits) => bits,
            Err(err) => return err.into_import_error(_py, &import_name),
        };
        let namespace = unsafe { runpy_namespace_from_module(_py, module_bits) };
        dec_ref_bits(_py, module_bits);
        match namespace {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn runpy_module_path_identity_is_deterministic() {
        assert_eq!(
            module_name_from_relative_path(std::path::Path::new("pkg/tool.py")).as_deref(),
            Some("pkg.tool")
        );
        assert_eq!(
            module_name_from_relative_path(std::path::Path::new("pkg/__init__.py")).as_deref(),
            Some("pkg")
        );
        assert_eq!(
            module_name_from_relative_path(std::path::Path::new("not-python.txt")),
            None
        );
        assert_eq!(
            module_name_from_relative_path(std::path::Path::new("pkg-name/tool-name.py"))
                .as_deref(),
            Some("pkg-name.tool-name")
        );
    }
}
