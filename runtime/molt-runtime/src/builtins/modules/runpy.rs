use super::*;

unsafe fn dict_copy_entries(_py: &PyToken<'_>, src_ptr: *mut u8, dst_ptr: *mut u8) {
    unsafe {
        let source_order = dict_order(src_ptr);
        for idx in (0..source_order.len()).step_by(2) {
            let key_bits = source_order[idx];
            let val_bits = source_order[idx + 1];
            dict_set_in_place(_py, dst_ptr, key_bits, val_bits);
        }
    }
}

unsafe fn runpy_import_module_bits(_py: &PyToken<'_>, name: &str) -> Result<u64, u64> {
    unsafe {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let name_ptr = alloc_string(_py, name.as_bytes());
            if name_ptr.is_null() {
                return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
            }
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let module_bits = molt_isolate_import(name_bits);
            let name_text =
                string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_else(|| name.to_string());
            let mut canonical_bits: Option<u64> = None;
            if !exception_pending(_py) {
                // Prefer canonical module handles from sys.modules when isolate-import
                // returns non-module payloads (status sentinels, accidental scalar values, etc).
                let sys_bits = {
                    let cache = crate::builtins::exceptions::internals::module_cache(_py);
                    let guard = cache.lock().unwrap();
                    guard.get("sys").copied()
                };
                if let Some(sys_bits) = sys_bits
                    && let Some(modules_ptr) = sys_modules_dict_ptr(_py, sys_bits)
                {
                    let module_key_ptr = alloc_string(_py, name_text.as_bytes());
                    if module_key_ptr.is_null() {
                        dec_ref_bits(_py, name_bits);
                        return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
                    }
                    let module_key_bits = MoltObject::from_ptr(module_key_ptr).bits();
                    let from_sys_bits = dict_get_in_place(_py, modules_ptr, module_key_bits);
                    dec_ref_bits(_py, module_key_bits);
                    if exception_pending(_py) {
                        dec_ref_bits(_py, name_bits);
                        return Err(MoltObject::none().bits());
                    }
                    if let Some(bits) = from_sys_bits
                        && let Some(ptr) = obj_from_bits(bits).as_ptr()
                    {
                        let ty = object_type_id(ptr);
                        if ty == TYPE_ID_MODULE || ty == TYPE_ID_DICT {
                            canonical_bits = Some(bits);
                        }
                    }
                }
                if canonical_bits.is_none() {
                    let cache = crate::builtins::exceptions::internals::module_cache(_py);
                    let guard = cache.lock().unwrap();
                    if let Some(bits) = guard.get(name)
                        && let Some(ptr) = obj_from_bits(*bits).as_ptr()
                    {
                        let ty = object_type_id(ptr);
                        if ty == TYPE_ID_MODULE || ty == TYPE_ID_DICT {
                            canonical_bits = Some(*bits);
                        }
                    }
                }
            }
            dec_ref_bits(_py, name_bits);

            if let Some(bits) = canonical_bits {
                if bits != module_bits {
                    if !obj_from_bits(module_bits).is_none() {
                        dec_ref_bits(_py, module_bits);
                    }
                    inc_ref_bits(_py, bits);
                }
                return Ok(bits);
            }

            if obj_from_bits(module_bits).is_none() && !exception_pending(_py) {
                if let Some(bits) = runpy_import_intrinsic_module(_py, &name_text)? {
                    return Ok(bits);
                }
                match runpy_import_via_builtins(_py, &name_text) {
                    Ok(Some(bits)) => return Ok(bits),
                    Ok(None) => {}
                    Err(err) => return Err(err),
                }
            }

            if let Some(ptr) = obj_from_bits(module_bits).as_ptr() {
                let ty = object_type_id(ptr);
                if ty != TYPE_ID_MODULE && ty != TYPE_ID_DICT {
                    let type_name = type_name(_py, obj_from_bits(module_bits));
                    dec_ref_bits(_py, module_bits);
                    return Err(raise_exception::<_>(
                        _py,
                        "TypeError",
                        &format!("runpy import returned non-module payload: {type_name}"),
                    ));
                }
            }
            Ok(module_bits)
        }
        #[cfg(target_arch = "wasm32")]
        {
            let cache = crate::builtins::exceptions::internals::module_cache(_py);
            let guard = cache.lock().unwrap();
            if let Some(bits) = guard.get(name) {
                inc_ref_bits(_py, *bits);
                Ok(*bits)
            } else {
                Ok(MoltObject::none().bits())
            }
        }
    }
}

unsafe fn runpy_import_intrinsic_module(_py: &PyToken<'_>, name: &str) -> Result<Option<u64>, u64> {
    unsafe {
        if name != "errno" {
            return Ok(None);
        }
        let name_ptr = alloc_string(_py, b"errno");
        if name_ptr.is_null() {
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let module_bits = molt_module_new(name_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, name_bits);
            return Err(MoltObject::none().bits());
        }
        let Some(module_ptr) = obj_from_bits(module_bits).as_ptr() else {
            dec_ref_bits(_py, name_bits);
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        };
        let module_dict_bits = module_dict_bits(module_ptr);
        let Some(module_dict_ptr) = obj_from_bits(module_dict_bits).as_ptr() else {
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, name_bits);
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "errno module dict unavailable",
            ));
        };
        if object_type_id(module_dict_ptr) != TYPE_ID_DICT {
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, name_bits);
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "errno module dict unavailable",
            ));
        }
        let payload_bits = crate::molt_errno_constants();
        if exception_pending(_py) {
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, name_bits);
            return Err(MoltObject::none().bits());
        }
        let Some(payload_ptr) = obj_from_bits(payload_bits).as_ptr() else {
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, name_bits);
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "errno constants unavailable",
            ));
        };
        if object_type_id(payload_ptr) != TYPE_ID_TUPLE {
            dec_ref_bits(_py, payload_bits);
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, name_bits);
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "errno constants unavailable",
            ));
        }
        let payload_items = seq_vec_ref(payload_ptr);
        if payload_items.len() != 2 {
            dec_ref_bits(_py, payload_bits);
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, name_bits);
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "errno constants unavailable",
            ));
        }
        let constants_bits = payload_items[0];
        let errorcode_bits = payload_items[1];
        let Some(constants_ptr) = obj_from_bits(constants_bits).as_ptr() else {
            dec_ref_bits(_py, payload_bits);
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, name_bits);
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "errno constants unavailable",
            ));
        };
        let Some(errorcode_ptr) = obj_from_bits(errorcode_bits).as_ptr() else {
            dec_ref_bits(_py, payload_bits);
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, name_bits);
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "errno constants unavailable",
            ));
        };
        if object_type_id(constants_ptr) != TYPE_ID_DICT
            || object_type_id(errorcode_ptr) != TYPE_ID_DICT
        {
            dec_ref_bits(_py, payload_bits);
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, name_bits);
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "errno constants unavailable",
            ));
        }
        dict_copy_entries(_py, constants_ptr, module_dict_ptr);
        if exception_pending(_py) {
            dec_ref_bits(_py, payload_bits);
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, name_bits);
            return Err(MoltObject::none().bits());
        }
        dict_set_str_key_bits(_py, module_dict_ptr, "errorcode", errorcode_bits)?;
        let cache_result = molt_module_cache_set(name_bits, module_bits);
        if obj_from_bits(cache_result).is_none() && exception_pending(_py) {
            dec_ref_bits(_py, payload_bits);
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, name_bits);
            return Err(MoltObject::none().bits());
        }
        dec_ref_bits(_py, payload_bits);
        dec_ref_bits(_py, name_bits);
        Ok(Some(module_bits))
    }
}

unsafe fn runpy_import_via_builtins(_py: &PyToken<'_>, name: &str) -> Result<Option<u64>, u64> {
    unsafe {
        let builtins_bits = {
            let cache = crate::builtins::exceptions::internals::module_cache(_py);
            let guard = cache.lock().unwrap();
            guard.get("builtins").copied()
        };
        let Some(builtins_bits) = builtins_bits else {
            return Ok(None);
        };
        let missing = missing_bits(_py);
        let import_name_bits = intern_static_name(
            _py,
            &modules_state(_py).runpy_import_dunder_name,
            b"__import__",
        );
        let import_bits = molt_getattr_builtin(builtins_bits, import_name_bits, missing);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        if is_missing_bits(_py, import_bits) || obj_from_bits(import_bits).is_none() {
            return Ok(None);
        }
        let callable = is_truthy(_py, obj_from_bits(molt_is_callable(import_bits)));
        if !callable {
            dec_ref_bits(_py, import_bits);
            return Ok(None);
        }
        let name_ptr = alloc_string(_py, name.as_bytes());
        if name_ptr.is_null() {
            dec_ref_bits(_py, import_bits);
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let none_bits = MoltObject::none().bits();
        // Non-empty fromlist keeps __import__ aligned with importlib for dotted
        // names (for example pkg.__main__), instead of returning only the
        // top-level package object.
        let fromlist_marker_ptr = alloc_string(_py, b"_molt_runpy");
        if fromlist_marker_ptr.is_null() {
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, import_bits);
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        let fromlist_marker_bits = MoltObject::from_ptr(fromlist_marker_ptr).bits();
        let fromlist_ptr = alloc_tuple(_py, &[fromlist_marker_bits]);
        if fromlist_ptr.is_null() {
            dec_ref_bits(_py, fromlist_marker_bits);
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, import_bits);
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        dec_ref_bits(_py, fromlist_marker_bits);
        let fromlist_bits = MoltObject::from_ptr(fromlist_ptr).bits();
        let zero_bits = int_bits_from_i64(_py, 0);
        let imported_bits = call_function_obj_vec(
            _py,
            import_bits,
            &[name_bits, none_bits, none_bits, fromlist_bits, zero_bits],
        );
        dec_ref_bits(_py, name_bits);
        dec_ref_bits(_py, fromlist_bits);
        dec_ref_bits(_py, import_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        if obj_from_bits(imported_bits).is_none() {
            return Ok(None);
        }
        if let Some(ptr) = obj_from_bits(imported_bits).as_ptr() {
            let ty = object_type_id(ptr);
            if ty == TYPE_ID_MODULE || ty == TYPE_ID_DICT {
                return Ok(Some(imported_bits));
            }
        }
        dec_ref_bits(_py, imported_bits);
        Ok(None)
    }
}

unsafe fn runpy_module_dict_ptr(_py: &PyToken<'_>, module_bits: u64) -> Result<*mut u8, u64> {
    unsafe {
        let module_ptr = match obj_from_bits(module_bits).as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_MODULE => ptr,
            Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => return Ok(ptr),
            _ => {
                let got = type_name(_py, obj_from_bits(module_bits));
                let msg = format!("module import expects module or dict, got {got}");
                return Err(raise_exception::<_>(_py, "TypeError", &msg));
            }
        };
        match obj_from_bits(module_dict_bits(module_ptr)).as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => Ok(ptr),
            _ => Err(raise_exception::<_>(
                _py,
                "TypeError",
                "module dict missing",
            )),
        }
    }
}

unsafe fn runpy_module_is_package(_py: &PyToken<'_>, module_dict_ptr: *mut u8) -> bool {
    unsafe {
        let path_name = intern_static_name(_py, &modules_state(_py).module_path_name, b"__path__");
        if let Some(bits) = dict_get_in_place(_py, module_dict_ptr, path_name) {
            return !obj_from_bits(bits).is_none();
        }
        false
    }
}

unsafe fn runpy_apply_module_metadata(
    _py: &PyToken<'_>,
    module_dict_ptr: *mut u8,
    out_ptr: *mut u8,
    target_name: &str,
) -> Result<(), u64> {
    unsafe {
        let target_name_ptr = alloc_string(_py, target_name.as_bytes());
        if target_name_ptr.is_null() {
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        let target_name_bits = MoltObject::from_ptr(target_name_ptr).bits();
        dict_set_str_key_bits(_py, out_ptr, "__name__", target_name_bits)?;
        dec_ref_bits(_py, target_name_bits);

        let module_state = modules_state(_py);
        let specials: [(&str, &[u8], &AtomicU64); 6] = [
            ("__file__", b"__file__", &module_state.module_file_name),
            (
                "__package__",
                b"__package__",
                &module_state.module_package_name,
            ),
            (
                "__cached__",
                b"__cached__",
                &module_state.module_cached_name,
            ),
            ("__spec__", b"__spec__", &module_state.module_spec_name),
            ("__doc__", b"__doc__", &module_state.module_doc_name),
            (
                "__loader__",
                b"__loader__",
                &module_state.module_loader_name,
            ),
        ];
        for (public_name, interned, slot) in specials {
            let key_bits = intern_static_name(_py, slot, interned);
            let value_bits = dict_get_in_place(_py, module_dict_ptr, key_bits)
                .unwrap_or_else(|| MoltObject::none().bits());
            dict_set_str_key_bits(_py, out_ptr, public_name, value_bits)?;
        }

        // Keep __name__ lookup warm for metadata reads in repeated runs.
        let _ = intern_static_name(_py, &module_state.module_name_name, b"__name__");
        Ok(())
    }
}

fn runpy_package_name(run_name: &str) -> String {
    run_name
        .rsplit_once('.')
        .map(|(prefix, _)| prefix.to_string())
        .unwrap_or_default()
}

unsafe fn runpy_sys_path_entries(_py: &PyToken<'_>) -> Vec<String> {
    unsafe {
        let sys_bits = {
            let cache = crate::builtins::exceptions::internals::module_cache(_py);
            let guard = cache.lock().unwrap();
            guard.get("sys").copied()
        };
        let Some(sys_bits) = sys_bits else {
            return Vec::new();
        };
        let Some(sys_ptr) = obj_from_bits(sys_bits).as_ptr() else {
            return Vec::new();
        };
        if object_type_id(sys_ptr) != TYPE_ID_MODULE {
            return Vec::new();
        }
        let path_key_ptr = alloc_string(_py, b"path");
        if path_key_ptr.is_null() {
            return Vec::new();
        }
        let path_key_bits = MoltObject::from_ptr(path_key_ptr).bits();
        let path_bits = module_attr_lookup(_py, sys_ptr, path_key_bits);
        dec_ref_bits(_py, path_key_bits);
        let Some(path_bits) = path_bits else {
            return Vec::new();
        };
        let Some(path_ptr) = obj_from_bits(path_bits).as_ptr() else {
            dec_ref_bits(_py, path_bits);
            return Vec::new();
        };
        let entries = seq_vec_ref(path_ptr)
            .iter()
            .filter_map(|&bits| string_obj_to_owned(obj_from_bits(bits)))
            .collect::<Vec<_>>();
        dec_ref_bits(_py, path_bits);
        entries
    }
}

fn runpy_normalize_candidate(path: PathBuf) -> String {
    std::fs::canonicalize(&path)
        .unwrap_or(path)
        .to_string_lossy()
        .into_owned()
}

// ── VFS-aware filesystem helpers (Plan B v0.1) ──────────────────────
// These check the VFS mount table first and fall back to the real
// filesystem, so `import mymodule` finds `/bundle/mymodule.py` when
// `/bundle` is in `sys.path`.

/// Returns `true` if `path` is a file in the VFS or on disk.
fn vfs_is_file(path: &std::path::Path) -> bool {
    let path_str = path.to_string_lossy();
    if let Some(state) = crate::runtime_state_for_gil()
        && let Some(vfs) = state.get_vfs()
        && let Some((_prefix, backend, rel)) = vfs.resolve(&path_str)
    {
        return match backend.stat(&rel) {
            Ok(st) => !st.is_dir,
            Err(_) => false,
        };
    }
    path.is_file()
}

/// Read a file to `String`, trying VFS first then the real filesystem.
#[allow(dead_code)]
fn vfs_read_to_string(path: &std::path::Path) -> Option<String> {
    let path_str = path.to_string_lossy();
    if let Some(state) = crate::runtime_state_for_gil()
        && let Some(vfs) = state.get_vfs()
        && let Some((_prefix, backend, rel)) = vfs.resolve(&path_str)
    {
        return match backend.open_read(&rel) {
            Ok(bytes) => String::from_utf8(bytes).ok(),
            Err(_) => None,
        };
    }
    std::fs::read_to_string(path).ok()
}

/// Read a file to raw bytes, trying VFS first then the real filesystem.
fn vfs_read(path: &str) -> std::io::Result<Vec<u8>> {
    if let Some(state) = crate::runtime_state_for_gil()
        && let Some(vfs) = state.get_vfs()
        && let Some((_prefix, backend, rel)) = vfs.resolve(path)
    {
        return backend.open_read(&rel).map_err(|e| {
            let kind = match e {
                crate::vfs::VfsError::NotFound => std::io::ErrorKind::NotFound,
                crate::vfs::VfsError::PermissionDenied
                | crate::vfs::VfsError::ReadOnly
                | crate::vfs::VfsError::CapabilityDenied(_) => std::io::ErrorKind::PermissionDenied,
                crate::vfs::VfsError::IsDirectory => std::io::ErrorKind::IsADirectory,
                _ => std::io::ErrorKind::Other,
            };
            std::io::Error::new(kind, e.to_string())
        });
    }
    std::fs::read(path)
}

fn runpy_resolve_module_source(
    mod_name: &str,
    sys_path: &[String],
) -> Option<(String, String, String)> {
    let parts = mod_name.split('.').collect::<Vec<_>>();
    if parts.is_empty() {
        return None;
    }
    for base in sys_path {
        let mut cur = if base.is_empty() {
            PathBuf::from(".")
        } else {
            PathBuf::from(base)
        };
        let mut matched = true;
        for (idx, part) in parts.iter().enumerate() {
            let last = idx + 1 == parts.len();
            if last {
                let file_path = cur.join(format!("{part}.py"));
                if vfs_is_file(&file_path) {
                    let package_name = runpy_package_name(mod_name);
                    return Some((
                        runpy_normalize_candidate(file_path),
                        mod_name.to_string(),
                        package_name,
                    ));
                }
                let pkg_dir = cur.join(part);
                let init_path = pkg_dir.join("__init__.py");
                if vfs_is_file(&init_path) {
                    let main_path = pkg_dir.join("__main__.py");
                    if vfs_is_file(&main_path) {
                        return Some((
                            runpy_normalize_candidate(main_path),
                            format!("{mod_name}.__main__"),
                            mod_name.to_string(),
                        ));
                    }
                }
                matched = false;
            } else {
                cur.push(part);
                if !vfs_is_file(&cur.join("__init__.py")) {
                    matched = false;
                    break;
                }
            }
        }
        if matched {
            continue;
        }
    }
    None
}

unsafe fn runpy_make_spec_obj(_py: &PyToken<'_>, import_name: &str) -> Result<u64, u64> {
    unsafe {
        let name_ptr = alloc_string(_py, import_name.as_bytes());
        if name_ptr.is_null() {
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let spec_ptr = alloc_module_obj(_py, name_bits);
        if spec_ptr.is_null() {
            dec_ref_bits(_py, name_bits);
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        let spec_bits = MoltObject::from_ptr(spec_ptr).bits();
        let dict_ptr = match obj_from_bits(module_dict_bits(spec_ptr)).as_ptr() {
            Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
            _ => {
                dec_ref_bits(_py, spec_bits);
                dec_ref_bits(_py, name_bits);
                return Err(raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "spec dict missing",
                ));
            }
        };
        dict_set_str_key_bits(_py, dict_ptr, "name", name_bits)?;
        dec_ref_bits(_py, name_bits);
        Ok(spec_bits)
    }
}

unsafe fn runpy_apply_source_metadata(
    _py: &PyToken<'_>,
    out_ptr: *mut u8,
    target_name: &str,
    import_name: &str,
    source_path: &str,
    package_name: &str,
) -> Result<(), u64> {
    unsafe {
        let run_name_ptr = alloc_string(_py, target_name.as_bytes());
        if run_name_ptr.is_null() {
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        let run_name_bits = MoltObject::from_ptr(run_name_ptr).bits();
        dict_set_str_key_bits(_py, out_ptr, "__name__", run_name_bits)?;
        dec_ref_bits(_py, run_name_bits);

        let path_ptr = alloc_string(_py, source_path.as_bytes());
        if path_ptr.is_null() {
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        let path_bits = MoltObject::from_ptr(path_ptr).bits();
        dict_set_str_key_bits(_py, out_ptr, "__file__", path_bits)?;
        dec_ref_bits(_py, path_bits);

        let pkg_ptr = alloc_string(_py, package_name.as_bytes());
        if pkg_ptr.is_null() {
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        let pkg_bits = MoltObject::from_ptr(pkg_ptr).bits();
        dict_set_str_key_bits(_py, out_ptr, "__package__", pkg_bits)?;
        dec_ref_bits(_py, pkg_bits);

        let none_bits = MoltObject::none().bits();
        dict_set_str_key_bits(_py, out_ptr, "__cached__", none_bits)?;
        dict_set_str_key_bits(_py, out_ptr, "__doc__", none_bits)?;
        dict_set_str_key_bits(_py, out_ptr, "__loader__", none_bits)?;

        let spec_bits = runpy_make_spec_obj(_py, import_name)?;
        dict_set_str_key_bits(_py, out_ptr, "__spec__", spec_bits)?;
        dec_ref_bits(_py, spec_bits);
        Ok(())
    }
}

fn is_ascii_digits(text: &str) -> bool {
    !text.is_empty() && text.as_bytes().iter().all(u8::is_ascii_digit)
}

fn is_identifier_text(text: &str) -> bool {
    let mut chars = text.chars();
    let Some(first) = chars.next() else {
        return false;
    };
    if first != '_' && !is_xid_start(first) {
        return false;
    }
    chars.all(|ch| ch == '_' || is_xid_continue(ch))
}

fn is_dotted_identifier_text(text: &str) -> bool {
    !text.is_empty()
        && text
            .split('.')
            .all(|segment| !segment.is_empty() && is_identifier_text(segment))
}

fn strip_inline_comment_text(text: &str) -> &str {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (idx, ch) in text.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '#' => return text[..idx].trim_end(),
            _ => {}
        }
    }
    text.trim_end()
}

fn parse_restricted_import_item(part: &str) -> Option<(&str, Option<&str>)> {
    let trimmed = part.trim();
    if trimmed.is_empty() {
        return None;
    }
    if let Some((left, right)) = trimmed.rsplit_once(" as ") {
        let module_name = left.trim();
        let alias = right.trim();
        if !is_dotted_identifier_text(module_name) || !is_identifier_text(alias) {
            return None;
        }
        return Some((module_name, Some(alias)));
    }
    if !is_dotted_identifier_text(trimmed) {
        return None;
    }
    Some((trimmed, None))
}

unsafe fn runpy_restricted_import_stmt(
    _py: &PyToken<'_>,
    namespace_ptr: *mut u8,
    spec: &str,
    filename: &str,
) -> Result<(), u64> {
    unsafe {
        for part in spec.split(',') {
            let Some((module_name, alias)) = parse_restricted_import_item(part) else {
                return Err(raise_exception::<_>(
                    _py,
                    "NotImplementedError",
                    &format!("unsupported import statement in {filename}"),
                ));
            };
            let imported_bits = runpy_import_module_bits(_py, module_name)?;
            if obj_from_bits(imported_bits).is_none() {
                if !exception_pending(_py) {
                    let message = format!("No module named '{module_name}'");
                    return Err(raise_exception::<_>(_py, "ModuleNotFoundError", &message));
                }
                return Err(MoltObject::none().bits());
            }

            let mut bind_bits = imported_bits;
            let bind_name = if let Some(alias_name) = alias {
                alias_name
            } else if let Some((head, _)) = module_name.split_once('.') {
                let top_bits = runpy_import_module_bits(_py, head)?;
                if !obj_from_bits(top_bits).is_none() {
                    bind_bits = top_bits;
                    dec_ref_bits(_py, imported_bits);
                }
                head
            } else {
                module_name
            };
            dict_set_str_key_bits(_py, namespace_ptr, bind_name, bind_bits)?;
            if !obj_from_bits(bind_bits).is_none() {
                dec_ref_bits(_py, bind_bits);
            }
        }
        Ok(())
    }
}

unsafe fn runpy_restricted_from_import_stmt(
    _py: &PyToken<'_>,
    namespace_ptr: *mut u8,
    spec: &str,
    filename: &str,
) -> Result<(), u64> {
    unsafe {
        let Some((module_name_raw, targets_raw)) = spec.split_once(" import ") else {
            return Err(raise_exception::<_>(
                _py,
                "NotImplementedError",
                &format!("unsupported from-import statement in {filename}"),
            ));
        };
        let module_name = module_name_raw.trim();
        if !is_dotted_identifier_text(module_name) {
            return Err(raise_exception::<_>(
                _py,
                "NotImplementedError",
                &format!("unsupported from-import statement in {filename}"),
            ));
        }

        let module_bits = runpy_import_module_bits(_py, module_name)?;
        if obj_from_bits(module_bits).is_none() {
            if !exception_pending(_py) {
                let message = format!("No module named '{module_name}'");
                return Err(raise_exception::<_>(_py, "ModuleNotFoundError", &message));
            }
            return Err(MoltObject::none().bits());
        }

        for target in targets_raw.split(',') {
            let trimmed = target.trim();
            if trimmed.is_empty() {
                if !obj_from_bits(module_bits).is_none() {
                    dec_ref_bits(_py, module_bits);
                }
                return Err(raise_exception::<_>(
                    _py,
                    "NotImplementedError",
                    &format!("unsupported from-import statement in {filename}"),
                ));
            }
            if trimmed == "*" {
                if let Err(err) = runpy_restricted_import_star_into_namespace(
                    _py,
                    namespace_ptr,
                    module_bits,
                    module_name,
                ) {
                    if !obj_from_bits(module_bits).is_none() {
                        dec_ref_bits(_py, module_bits);
                    }
                    return Err(err);
                }
                continue;
            }
            let (name, alias) = if let Some((left, right)) = trimmed.rsplit_once(" as ") {
                let left = left.trim();
                let right = right.trim();
                if !is_identifier_text(left) || !is_identifier_text(right) {
                    if !obj_from_bits(module_bits).is_none() {
                        dec_ref_bits(_py, module_bits);
                    }
                    return Err(raise_exception::<_>(
                        _py,
                        "NotImplementedError",
                        &format!("unsupported from-import statement in {filename}"),
                    ));
                }
                (left, right)
            } else {
                if !is_identifier_text(trimmed) {
                    if !obj_from_bits(module_bits).is_none() {
                        dec_ref_bits(_py, module_bits);
                    }
                    return Err(raise_exception::<_>(
                        _py,
                        "NotImplementedError",
                        &format!("unsupported from-import statement in {filename}"),
                    ));
                }
                (trimmed, trimmed)
            };

            let name_ptr = alloc_string(_py, name.as_bytes());
            if name_ptr.is_null() {
                if !obj_from_bits(module_bits).is_none() {
                    dec_ref_bits(_py, module_bits);
                }
                return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
            }
            let name_bits = MoltObject::from_ptr(name_ptr).bits();
            let missing = missing_bits(_py);
            let value_bits = molt_getattr_builtin(module_bits, name_bits, missing);
            dec_ref_bits(_py, name_bits);
            if exception_pending(_py) {
                if !obj_from_bits(module_bits).is_none() {
                    dec_ref_bits(_py, module_bits);
                }
                return Err(MoltObject::none().bits());
            }
            if is_missing_bits(_py, value_bits) {
                if !obj_from_bits(value_bits).is_none() {
                    dec_ref_bits(_py, value_bits);
                }
                if !obj_from_bits(module_bits).is_none() {
                    dec_ref_bits(_py, module_bits);
                }
                let message = format!("cannot import name '{name}' from '{module_name}'");
                return Err(raise_exception::<_>(_py, "ImportError", &message));
            }

            dict_set_str_key_bits(_py, namespace_ptr, alias, value_bits)?;
            if !obj_from_bits(value_bits).is_none() {
                dec_ref_bits(_py, value_bits);
            }
        }

        if !obj_from_bits(module_bits).is_none() {
            dec_ref_bits(_py, module_bits);
        }
        Ok(())
    }
}

unsafe fn runpy_restricted_import_star_into_namespace(
    _py: &PyToken<'_>,
    namespace_ptr: *mut u8,
    module_bits: u64,
    module_name: &str,
) -> Result<(), u64> {
    unsafe {
        let import_star_error = || {
            let message = format!("cannot import name '*' from '{module_name}'");
            raise_exception::<u64>(_py, "ImportError", &message)
        };
        let mut module_obj_ptr = obj_from_bits(module_bits).as_ptr();
        let mut module_ty = module_obj_ptr.map(|ptr| object_type_id(ptr));
        if !matches!(module_ty, Some(TYPE_ID_MODULE | TYPE_ID_DICT)) {
            let sys_bits = {
                let cache = crate::builtins::exceptions::internals::module_cache(_py);
                let guard = cache.lock().unwrap();
                guard.get("sys").copied()
            };
            if let Some(sys_bits) = sys_bits
                && let Some(modules_ptr) = sys_modules_dict_ptr(_py, sys_bits)
            {
                let key_ptr = alloc_string(_py, module_name.as_bytes());
                if key_ptr.is_null() {
                    return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
                }
                let key_bits = MoltObject::from_ptr(key_ptr).bits();
                let from_sys_bits = dict_get_in_place(_py, modules_ptr, key_bits);
                dec_ref_bits(_py, key_bits);
                if exception_pending(_py) {
                    return Err(MoltObject::none().bits());
                }
                if let Some(bits) = from_sys_bits {
                    module_obj_ptr = obj_from_bits(bits).as_ptr();
                    module_ty = module_obj_ptr.map(|ptr| object_type_id(ptr));
                }
            }
        }
        let Some(module_obj_ptr) = module_obj_ptr else {
            return Err(import_star_error());
        };
        let module_ty = module_ty.unwrap_or_else(|| object_type_id(module_obj_ptr));
        let module_dict_ptr = if module_ty == TYPE_ID_MODULE {
            let module_dict_bits = module_dict_bits(module_obj_ptr);
            match obj_from_bits(module_dict_bits).as_ptr() {
                Some(ptr) if object_type_id(ptr) == TYPE_ID_DICT => ptr,
                _ => {
                    return Err(raise_exception::<_>(
                        _py,
                        "TypeError",
                        "module dict missing",
                    ));
                }
            }
        } else if module_ty == TYPE_ID_DICT {
            module_obj_ptr
        } else {
            return Err(import_star_error());
        };

        let all_name_bits =
            intern_static_name(_py, &runtime_state(_py).interned.all_name, b"__all__");
        if let Some(all_bits) = dict_get_in_place(_py, module_dict_ptr, all_name_bits) {
            let iter_bits = molt_iter(all_bits);
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }
            loop {
                let pair_bits = molt_iter_next(iter_bits);
                let Some(pair_ptr) = obj_from_bits(pair_bits).as_ptr() else {
                    return Err(MoltObject::none().bits());
                };
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    return Err(MoltObject::none().bits());
                }
                let elems = seq_vec_ref(pair_ptr);
                if elems.len() < 2 {
                    return Err(MoltObject::none().bits());
                }
                if is_truthy(_py, obj_from_bits(elems[1])) {
                    break;
                }
                let name_bits = elems[0];
                match obj_from_bits(name_bits).as_ptr() {
                    Some(name_ptr) if object_type_id(name_ptr) == TYPE_ID_STRING => {}
                    _ => {
                        let type_name = class_name_for_error(type_of_bits(_py, name_bits));
                        let message =
                            format!("Item in {module_name}.__all__ must be str, not {type_name}");
                        return Err(raise_exception::<_>(_py, "TypeError", &message));
                    }
                }
                let Some(value_bits) = dict_get_in_place(_py, module_dict_ptr, name_bits) else {
                    let name = string_obj_to_owned(obj_from_bits(name_bits)).unwrap_or_default();
                    let message = format!("module '{module_name}' has no attribute '{name}'");
                    return Err(raise_exception::<_>(_py, "AttributeError", &message));
                };
                dict_set_in_place(_py, namespace_ptr, name_bits, value_bits);
            }
            return Ok(());
        }

        let order = dict_order(module_dict_ptr);
        for idx in (0..order.len()).step_by(2) {
            let name_bits = order[idx];
            let Some(name_ptr) = obj_from_bits(name_bits).as_ptr() else {
                continue;
            };
            if object_type_id(name_ptr) != TYPE_ID_STRING {
                continue;
            }
            let name_len = string_len(name_ptr);
            if name_len > 0 {
                let name_bytes = std::slice::from_raw_parts(string_bytes(name_ptr), name_len);
                if name_bytes[0] == b'_' {
                    continue;
                }
            }
            let value_bits = order[idx + 1];
            dict_set_in_place(_py, namespace_ptr, name_bits, value_bits);
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq)]
enum RestrictedLiteral {
    NoneValue,
    Bool(bool),
    Int(i64),
    Float(f64),
    Str(String),
    Bytes(Vec<u8>),
    List(Vec<RestrictedLiteral>),
    Tuple(Vec<RestrictedLiteral>),
    Dict(Vec<(RestrictedLiteral, RestrictedLiteral)>),
}

#[derive(Debug, Clone, PartialEq)]
enum RestrictedReferenceIndex {
    Int(i64),
    Str(String),
}

#[derive(Debug, Clone, PartialEq)]
enum RestrictedReferenceStep {
    Attr(String),
    Index(RestrictedReferenceIndex),
}

#[derive(Debug, Clone, PartialEq)]
struct RestrictedReferenceExpr {
    base: String,
    steps: Vec<RestrictedReferenceStep>,
}

fn split_identifier_prefix(text: &str) -> Option<(&str, &str)> {
    let mut chars = text.char_indices();
    let (first_idx, first) = chars.next()?;
    debug_assert_eq!(first_idx, 0);
    if first != '_' && !is_xid_start(first) {
        return None;
    }

    let mut end = text.len();
    for (idx, ch) in chars {
        if ch == '_' || is_xid_continue(ch) {
            continue;
        }
        end = idx;
        break;
    }
    Some((&text[..end], &text[end..]))
}

fn split_restricted_reference_index(text: &str) -> Option<(&str, &str)> {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    for (idx, ch) in text.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            ']' => {
                let next = idx + ch.len_utf8();
                return Some((&text[..idx], &text[next..]));
            }
            _ => {}
        }
    }
    None
}

fn parse_restricted_string_literal(text: &str) -> Option<String> {
    let mut chars = text.chars();
    let quote = chars.next()?;
    if quote != '\'' && quote != '"' {
        return None;
    }
    if !text.ends_with(quote) || text.chars().count() < 2 {
        return None;
    }
    let inner = &text[1..text.len() - 1];
    let mut out = String::with_capacity(inner.len());
    let mut iter = inner.chars();
    while let Some(ch) = iter.next() {
        if ch != '\\' {
            out.push(ch);
            continue;
        }
        match iter.next() {
            Some('\\') => out.push('\\'),
            Some('n') => out.push('\n'),
            Some('t') => out.push('\t'),
            Some('r') => out.push('\r'),
            Some('\'') if quote == '\'' => out.push('\''),
            Some('"') if quote == '"' => out.push('"'),
            Some(other) => {
                out.push('\\');
                out.push(other);
            }
            None => out.push('\\'),
        }
    }
    Some(out)
}

fn split_top_level_parts(text: &str, delimiter: char) -> Option<Vec<&str>> {
    let mut parts: Vec<&str> = Vec::new();
    let mut start = 0usize;
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut round_depth = 0i32;
    let mut square_depth = 0i32;
    let mut curly_depth = 0i32;
    for (idx, ch) in text.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '(' => round_depth += 1,
            ')' => {
                round_depth -= 1;
                if round_depth < 0 {
                    return None;
                }
            }
            '[' => square_depth += 1,
            ']' => {
                square_depth -= 1;
                if square_depth < 0 {
                    return None;
                }
            }
            '{' => curly_depth += 1,
            '}' => {
                curly_depth -= 1;
                if curly_depth < 0 {
                    return None;
                }
            }
            _ => {}
        }
        if ch == delimiter && round_depth == 0 && square_depth == 0 && curly_depth == 0 {
            parts.push(&text[start..idx]);
            start = idx + ch.len_utf8();
        }
    }
    if quote.is_some() || round_depth != 0 || square_depth != 0 || curly_depth != 0 {
        return None;
    }
    parts.push(&text[start..]);
    Some(parts)
}

fn split_top_level_once(text: &str, delimiter: char) -> Option<(&str, &str)> {
    let mut quote: Option<char> = None;
    let mut escaped = false;
    let mut round_depth = 0i32;
    let mut square_depth = 0i32;
    let mut curly_depth = 0i32;
    for (idx, ch) in text.char_indices() {
        if let Some(active_quote) = quote {
            if escaped {
                escaped = false;
                continue;
            }
            if ch == '\\' {
                escaped = true;
                continue;
            }
            if ch == active_quote {
                quote = None;
            }
            continue;
        }
        match ch {
            '\'' | '"' => quote = Some(ch),
            '(' => round_depth += 1,
            ')' => {
                round_depth -= 1;
                if round_depth < 0 {
                    return None;
                }
            }
            '[' => square_depth += 1,
            ']' => {
                square_depth -= 1;
                if square_depth < 0 {
                    return None;
                }
            }
            '{' => curly_depth += 1,
            '}' => {
                curly_depth -= 1;
                if curly_depth < 0 {
                    return None;
                }
            }
            _ => {}
        }
        if ch == delimiter && round_depth == 0 && square_depth == 0 && curly_depth == 0 {
            let split_at = idx + ch.len_utf8();
            return Some((&text[..idx], &text[split_at..]));
        }
    }
    None
}

fn parse_restricted_bytes_literal(text: &str) -> Option<Vec<u8>> {
    let rest = text.strip_prefix('b').or_else(|| text.strip_prefix('B'))?;
    parse_restricted_string_literal(rest).map(|value| value.into_bytes())
}

fn parse_restricted_reference_index(text: &str) -> Option<RestrictedReferenceIndex> {
    match parse_restricted_literal(text.trim())? {
        RestrictedLiteral::Int(value) => Some(RestrictedReferenceIndex::Int(value)),
        RestrictedLiteral::Str(value) => Some(RestrictedReferenceIndex::Str(value)),
        _ => None,
    }
}

fn parse_restricted_reference_expr(text: &str) -> Option<RestrictedReferenceExpr> {
    let mut rest = text.trim();
    let (base, tail) = split_identifier_prefix(rest)?;
    let mut steps: Vec<RestrictedReferenceStep> = Vec::new();
    rest = tail;

    loop {
        rest = rest.trim_start();
        if rest.is_empty() {
            break;
        }
        if let Some(after_dot) = rest.strip_prefix('.') {
            let after_dot = after_dot.trim_start();
            let (attr, tail) = split_identifier_prefix(after_dot)?;
            if !is_identifier_text(attr) {
                return None;
            }
            steps.push(RestrictedReferenceStep::Attr(attr.to_string()));
            rest = tail;
            continue;
        }
        if let Some(after_open) = rest.strip_prefix('[') {
            let (inner, tail) = split_restricted_reference_index(after_open)?;
            let index = parse_restricted_reference_index(inner)?;
            steps.push(RestrictedReferenceStep::Index(index));
            rest = tail;
            continue;
        }
        return None;
    }

    Some(RestrictedReferenceExpr {
        base: base.to_string(),
        steps,
    })
}

fn parse_restricted_literal(text: &str) -> Option<RestrictedLiteral> {
    let text = text.trim();
    if text.is_empty() {
        return None;
    }
    match text {
        "None" => return Some(RestrictedLiteral::NoneValue),
        "True" => return Some(RestrictedLiteral::Bool(true)),
        "False" => return Some(RestrictedLiteral::Bool(false)),
        _ => {}
    }
    if let Some(rest) = text.strip_prefix('+')
        && is_ascii_digits(rest)
    {
        return rest.parse::<i64>().ok().map(RestrictedLiteral::Int);
    }
    if let Some(rest) = text.strip_prefix('-')
        && is_ascii_digits(rest)
    {
        return text.parse::<i64>().ok().map(RestrictedLiteral::Int);
    }
    if is_ascii_digits(text) {
        return text.parse::<i64>().ok().map(RestrictedLiteral::Int);
    }
    if (text.contains('.') || text.contains('e') || text.contains('E'))
        && let Ok(value) = text.parse::<f64>()
    {
        return Some(RestrictedLiteral::Float(value));
    }
    if let Some(value) = parse_restricted_bytes_literal(text) {
        return Some(RestrictedLiteral::Bytes(value));
    }
    if text.starts_with('[') && text.ends_with(']') {
        let inner = text[1..text.len() - 1].trim();
        if inner.is_empty() {
            return Some(RestrictedLiteral::List(Vec::new()));
        }
        let parts = split_top_level_parts(inner, ',')?;
        let mut values: Vec<RestrictedLiteral> = Vec::new();
        for part in parts {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            values.push(parse_restricted_literal(part)?);
        }
        return Some(RestrictedLiteral::List(values));
    }
    if text.starts_with('(') && text.ends_with(')') {
        let inner = text[1..text.len() - 1].trim();
        if inner.is_empty() {
            return Some(RestrictedLiteral::Tuple(Vec::new()));
        }
        let parts = split_top_level_parts(inner, ',')?;
        let has_top_level_comma = parts.len() > 1;
        let mut values: Vec<RestrictedLiteral> = Vec::new();
        for part in parts {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            values.push(parse_restricted_literal(part)?);
        }
        if !has_top_level_comma && values.len() == 1 {
            return values.into_iter().next();
        }
        return Some(RestrictedLiteral::Tuple(values));
    }
    if text.starts_with('{') && text.ends_with('}') {
        let inner = text[1..text.len() - 1].trim();
        if inner.is_empty() {
            return Some(RestrictedLiteral::Dict(Vec::new()));
        }
        let parts = split_top_level_parts(inner, ',')?;
        let mut entries: Vec<(RestrictedLiteral, RestrictedLiteral)> = Vec::new();
        for part in parts {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            let (left, right) = split_top_level_once(part, ':')?;
            let key = parse_restricted_literal(left.trim())?;
            let value = parse_restricted_literal(right.trim())?;
            entries.push((key, value));
        }
        return Some(RestrictedLiteral::Dict(entries));
    }
    parse_restricted_string_literal(text).map(RestrictedLiteral::Str)
}

fn restricted_literal_truthy(value: &RestrictedLiteral) -> bool {
    match value {
        RestrictedLiteral::NoneValue => false,
        RestrictedLiteral::Bool(flag) => *flag,
        RestrictedLiteral::Int(v) => *v != 0,
        RestrictedLiteral::Float(v) => *v != 0.0,
        RestrictedLiteral::Str(v) => !v.is_empty(),
        RestrictedLiteral::Bytes(v) => !v.is_empty(),
        RestrictedLiteral::List(v) => !v.is_empty(),
        RestrictedLiteral::Tuple(v) => !v.is_empty(),
        RestrictedLiteral::Dict(v) => !v.is_empty(),
    }
}

fn restricted_literal_to_bits(_py: &PyToken<'_>, value: RestrictedLiteral) -> Result<u64, u64> {
    match value {
        RestrictedLiteral::NoneValue => Ok(MoltObject::none().bits()),
        RestrictedLiteral::Bool(flag) => Ok(MoltObject::from_bool(flag).bits()),
        RestrictedLiteral::Int(value) => Ok(int_bits_from_i64(_py, value)),
        RestrictedLiteral::Float(value) => Ok(MoltObject::from_float(value).bits()),
        RestrictedLiteral::Str(value) => {
            let ptr = alloc_string(_py, value.as_bytes());
            if ptr.is_null() {
                Err(raise_exception::<_>(_py, "MemoryError", "out of memory"))
            } else {
                Ok(MoltObject::from_ptr(ptr).bits())
            }
        }
        RestrictedLiteral::Bytes(value) => {
            let ptr = alloc_bytes(_py, value.as_slice());
            if ptr.is_null() {
                Err(raise_exception::<_>(_py, "MemoryError", "out of memory"))
            } else {
                Ok(MoltObject::from_ptr(ptr).bits())
            }
        }
        RestrictedLiteral::List(values) => {
            let mut bits_vec: Vec<u64> = Vec::with_capacity(values.len());
            for item in values {
                bits_vec.push(restricted_literal_to_bits(_py, item)?);
            }
            let ptr = alloc_list(_py, bits_vec.as_slice());
            for bits in bits_vec {
                if !obj_from_bits(bits).is_none() {
                    dec_ref_bits(_py, bits);
                }
            }
            if ptr.is_null() {
                Err(raise_exception::<_>(_py, "MemoryError", "out of memory"))
            } else {
                Ok(MoltObject::from_ptr(ptr).bits())
            }
        }
        RestrictedLiteral::Tuple(values) => {
            let mut bits_vec: Vec<u64> = Vec::with_capacity(values.len());
            for item in values {
                bits_vec.push(restricted_literal_to_bits(_py, item)?);
            }
            let ptr = alloc_tuple(_py, bits_vec.as_slice());
            for bits in bits_vec {
                if !obj_from_bits(bits).is_none() {
                    dec_ref_bits(_py, bits);
                }
            }
            if ptr.is_null() {
                Err(raise_exception::<_>(_py, "MemoryError", "out of memory"))
            } else {
                Ok(MoltObject::from_ptr(ptr).bits())
            }
        }
        RestrictedLiteral::Dict(entries) => {
            let mut pairs: Vec<u64> = Vec::with_capacity(entries.len() * 2);
            for (key, value) in entries {
                pairs.push(restricted_literal_to_bits(_py, key)?);
                pairs.push(restricted_literal_to_bits(_py, value)?);
            }
            let ptr = alloc_dict_with_pairs(_py, &pairs);
            for bits in pairs {
                if !obj_from_bits(bits).is_none() {
                    dec_ref_bits(_py, bits);
                }
            }
            if ptr.is_null() {
                Err(raise_exception::<_>(_py, "MemoryError", "out of memory"))
            } else {
                Ok(MoltObject::from_ptr(ptr).bits())
            }
        }
    }
}

unsafe fn runpy_restricted_namespace_lookup_bits(
    _py: &PyToken<'_>,
    namespace_ptr: *mut u8,
    name: &str,
) -> Result<u64, u64> {
    unsafe {
        let name_ptr = alloc_string(_py, name.as_bytes());
        if name_ptr.is_null() {
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let value_bits = dict_get_in_place(_py, namespace_ptr, name_bits);
        dec_ref_bits(_py, name_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        let Some(value_bits) = value_bits else {
            let msg = format!("name '{name}' is not defined");
            return Err(raise_exception::<_>(_py, "NameError", &msg));
        };
        inc_ref_bits(_py, value_bits);
        Ok(value_bits)
    }
}

unsafe fn runpy_restricted_reference_attr_bits(
    _py: &PyToken<'_>,
    value_bits: u64,
    attr: &str,
) -> Result<u64, u64> {
    let attr_ptr = alloc_string(_py, attr.as_bytes());
    if attr_ptr.is_null() {
        return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
    }
    let attr_bits = MoltObject::from_ptr(attr_ptr).bits();
    let missing = missing_bits(_py);
    let out_bits = molt_getattr_builtin(value_bits, attr_bits, missing);
    dec_ref_bits(_py, attr_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if is_missing_bits(_py, out_bits) {
        if !obj_from_bits(out_bits).is_none() {
            dec_ref_bits(_py, out_bits);
        }
        let type_name = class_name_for_error(type_of_bits(_py, value_bits));
        let msg = format!("'{type_name}' object has no attribute '{attr}'");
        return Err(raise_exception::<_>(_py, "AttributeError", &msg));
    }
    Ok(out_bits)
}

unsafe fn runpy_restricted_reference_index_key_bits(
    _py: &PyToken<'_>,
    index: &RestrictedReferenceIndex,
) -> Result<u64, u64> {
    match index {
        RestrictedReferenceIndex::Int(value) => Ok(int_bits_from_i64(_py, *value)),
        RestrictedReferenceIndex::Str(value) => {
            let ptr = alloc_string(_py, value.as_bytes());
            if ptr.is_null() {
                Err(raise_exception::<_>(_py, "MemoryError", "out of memory"))
            } else {
                Ok(MoltObject::from_ptr(ptr).bits())
            }
        }
    }
}

unsafe fn runpy_restricted_reference_subscript_bits(
    _py: &PyToken<'_>,
    value_bits: u64,
    index: &RestrictedReferenceIndex,
) -> Result<u64, u64> {
    unsafe {
        let key_bits = runpy_restricted_reference_index_key_bits(_py, index)?;
        let out_bits = crate::molt_index(value_bits, key_bits);
        if !obj_from_bits(key_bits).is_none() {
            dec_ref_bits(_py, key_bits);
        }
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        Ok(out_bits)
    }
}

unsafe fn runpy_eval_restricted_reference_expr(
    _py: &PyToken<'_>,
    namespace_ptr: *mut u8,
    expr: &RestrictedReferenceExpr,
) -> Result<u64, u64> {
    unsafe {
        let mut current_bits =
            runpy_restricted_namespace_lookup_bits(_py, namespace_ptr, &expr.base)?;
        for step in &expr.steps {
            let next_bits = match step {
                RestrictedReferenceStep::Attr(attr) => {
                    runpy_restricted_reference_attr_bits(_py, current_bits, attr)
                }
                RestrictedReferenceStep::Index(index) => {
                    runpy_restricted_reference_subscript_bits(_py, current_bits, index)
                }
            };
            let next_bits = match next_bits {
                Ok(bits) => bits,
                Err(err) => {
                    if !obj_from_bits(current_bits).is_none() {
                        dec_ref_bits(_py, current_bits);
                    }
                    return Err(err);
                }
            };
            if !obj_from_bits(current_bits).is_none() {
                dec_ref_bits(_py, current_bits);
            }
            current_bits = next_bits;
        }
        Ok(current_bits)
    }
}

unsafe fn runpy_exec_restricted_stmt(
    _py: &PyToken<'_>,
    namespace_ptr: *mut u8,
    stripped: &str,
    filename: &str,
) -> Result<(), u64> {
    unsafe {
        if stripped == "pass" {
            return Ok(());
        }
        if let Some(rest) = stripped.strip_prefix("import ") {
            runpy_restricted_import_stmt(_py, namespace_ptr, rest.trim(), filename)?;
            return Ok(());
        }
        if let Some(rest) = stripped.strip_prefix("from ") {
            runpy_restricted_from_import_stmt(_py, namespace_ptr, rest.trim(), filename)?;
            return Ok(());
        }
        if !stripped.contains('=') || stripped.contains("==") || stripped.contains("!=") {
            return Err(raise_exception::<_>(
                _py,
                "NotImplementedError",
                &format!("unsupported module statement in {filename}"),
            ));
        }
        let Some((left, right)) = stripped.split_once('=') else {
            return Err(raise_exception::<_>(
                _py,
                "NotImplementedError",
                &format!("unsupported module statement in {filename}"),
            ));
        };
        let target = left.trim();
        if !is_identifier_text(target) {
            return Err(raise_exception::<_>(
                _py,
                "NotImplementedError",
                &format!("unsupported assignment target in {filename}"),
            ));
        }
        let rhs = right.trim();
        let value_bits = if let Some(value) = parse_restricted_literal(rhs) {
            restricted_literal_to_bits(_py, value)?
        } else if let Some(reference) = parse_restricted_reference_expr(rhs) {
            runpy_eval_restricted_reference_expr(_py, namespace_ptr, &reference)?
        } else {
            return Err(raise_exception::<_>(
                _py,
                "NotImplementedError",
                &format!("unsupported assignment in {filename}"),
            ));
        };
        dict_set_str_key_bits(_py, namespace_ptr, target, value_bits)?;
        dec_ref_bits(_py, value_bits);
        Ok(())
    }
}

pub(crate) unsafe fn runpy_exec_restricted_source(
    _py: &PyToken<'_>,
    namespace_ptr: *mut u8,
    source: &str,
    filename: &str,
) -> Result<(), u64> {
    unsafe {
        let lines: Vec<&str> = source.lines().collect();
        let mut idx = 0usize;
        let mut saw_stmt = false;
        while idx < lines.len() {
            let raw = lines[idx];
            idx += 1;
            let stripped = strip_inline_comment_text(raw.trim());
            if stripped.is_empty() || stripped.starts_with('#') {
                continue;
            }
            if !saw_stmt && (stripped.starts_with("\"\"\"") || stripped.starts_with("'''")) {
                let quote = &stripped[..3];
                let doc = if stripped.ends_with(quote) && stripped.len() > 6 {
                    stripped[3..stripped.len() - 3].to_string()
                } else {
                    let mut doc_lines: Vec<String> = vec![stripped[3..].to_string()];
                    while idx < lines.len() {
                        let chunk = lines[idx];
                        idx += 1;
                        if let Some(end) = chunk.find(quote) {
                            doc_lines.push(chunk[..end].to_string());
                            break;
                        }
                        doc_lines.push(chunk.to_string());
                    }
                    doc_lines.join("\n")
                };
                let doc_ptr = alloc_string(_py, doc.as_bytes());
                if doc_ptr.is_null() {
                    return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
                }
                let doc_bits = MoltObject::from_ptr(doc_ptr).bits();
                dict_set_str_key_bits(_py, namespace_ptr, "__doc__", doc_bits)?;
                dec_ref_bits(_py, doc_bits);
                saw_stmt = true;
                continue;
            }

            saw_stmt = true;
            if let Some(cond_raw) = stripped
                .strip_prefix("if ")
                .and_then(|rest| rest.strip_suffix(':'))
            {
                let condition = parse_restricted_literal(cond_raw.trim()).ok_or_else(|| {
                    raise_exception::<u64>(
                        _py,
                        "NotImplementedError",
                        &format!("unsupported module statement in {filename}"),
                    )
                })?;
                let cond_true = restricted_literal_truthy(&condition);
                let current_indent = raw
                    .chars()
                    .take_while(|ch| *ch == ' ' || *ch == '\t')
                    .count();
                let mut saw_indented_stmt = false;
                while idx < lines.len() {
                    let block_raw = lines[idx];
                    let block_indent = block_raw
                        .chars()
                        .take_while(|ch| *ch == ' ' || *ch == '\t')
                        .count();
                    let block_trimmed = strip_inline_comment_text(block_raw.trim());
                    if !block_trimmed.is_empty() && block_indent <= current_indent {
                        break;
                    }
                    idx += 1;
                    if block_trimmed.is_empty() || block_trimmed.starts_with('#') {
                        continue;
                    }
                    if block_indent <= current_indent {
                        continue;
                    }
                    saw_indented_stmt = true;
                    if !cond_true {
                        continue;
                    }
                    runpy_exec_restricted_stmt(_py, namespace_ptr, block_trimmed, filename)?;
                }
                if !saw_indented_stmt {
                    return Err(raise_exception::<_>(
                        _py,
                        "NotImplementedError",
                        &format!("unsupported module statement in {filename}"),
                    ));
                }
                continue;
            }
            runpy_exec_restricted_stmt(_py, namespace_ptr, stripped, filename)?;
        }
        Ok(())
    }
}

struct RunpySysModulesSwapState {
    modules_ptr: *mut u8,
    key_bits: u64,
    previous_bits: Option<u64>,
}

struct RunpyArgv0SwapState {
    previous_arg0_bits: u64,
}

fn runpy_sys_module_bits(_py: &PyToken<'_>) -> Option<u64> {
    let cache = crate::builtins::exceptions::internals::module_cache(_py);
    let guard = cache.lock().unwrap();
    guard.get("sys").copied()
}

unsafe fn runpy_begin_sys_modules_swap(
    _py: &PyToken<'_>,
    run_name: &str,
    module_bits: u64,
) -> Result<Option<RunpySysModulesSwapState>, u64> {
    unsafe {
        let Some(sys_bits) = runpy_sys_module_bits(_py) else {
            return Ok(None);
        };
        let Some(modules_ptr) = sys_modules_dict_ptr(_py, sys_bits) else {
            return Ok(None);
        };
        let run_name_ptr = alloc_string(_py, run_name.as_bytes());
        if run_name_ptr.is_null() {
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        let run_name_bits = MoltObject::from_ptr(run_name_ptr).bits();
        let previous_bits = dict_get_in_place(_py, modules_ptr, run_name_bits);
        if exception_pending(_py) {
            dec_ref_bits(_py, run_name_bits);
            return Err(MoltObject::none().bits());
        }
        if let Some(bits) = previous_bits {
            inc_ref_bits(_py, bits);
        }
        dict_set_in_place(_py, modules_ptr, run_name_bits, module_bits);
        if exception_pending(_py) {
            if let Some(bits) = previous_bits {
                dec_ref_bits(_py, bits);
            }
            dec_ref_bits(_py, run_name_bits);
            return Err(MoltObject::none().bits());
        }
        Ok(Some(RunpySysModulesSwapState {
            modules_ptr,
            key_bits: run_name_bits,
            previous_bits,
        }))
    }
}

unsafe fn runpy_restore_sys_modules_swap(
    _py: &PyToken<'_>,
    state: &mut Option<RunpySysModulesSwapState>,
) {
    unsafe {
        let Some(state) = state.take() else {
            return;
        };
        let saved_exc_bits = if exception_pending(_py) {
            let bits = molt_exception_last();
            clear_exception(_py);
            Some(bits)
        } else {
            None
        };
        if let Some(bits) = state.previous_bits {
            dict_set_in_place(_py, state.modules_ptr, state.key_bits, bits);
            dec_ref_bits(_py, bits);
        } else {
            dict_del_in_place(_py, state.modules_ptr, state.key_bits);
        }
        dec_ref_bits(_py, state.key_bits);
        if let Some(saved_bits) = saved_exc_bits {
            if exception_pending(_py) {
                clear_exception(_py);
            }
            if !obj_from_bits(saved_bits).is_none() {
                let _ = crate::molt_exception_set_last(saved_bits);
            }
            dec_ref_bits(_py, saved_bits);
        }
    }
}

unsafe fn runpy_sys_argv_list_bits(_py: &PyToken<'_>) -> Result<Option<u64>, u64> {
    unsafe {
        let Some(sys_bits) = runpy_sys_module_bits(_py) else {
            return Ok(None);
        };
        let Some(sys_ptr) = obj_from_bits(sys_bits).as_ptr() else {
            return Ok(None);
        };
        if object_type_id(sys_ptr) != TYPE_ID_MODULE {
            return Ok(None);
        }
        let argv_name_bits = intern_static_name(_py, &modules_state(_py).sys_argv_name, b"argv");
        if obj_from_bits(argv_name_bits).is_none() {
            return if exception_pending(_py) {
                Err(MoltObject::none().bits())
            } else {
                Ok(None)
            };
        }
        let Some(argv_bits) = module_attr_lookup(_py, sys_ptr, argv_name_bits) else {
            if exception_pending(_py) {
                if clear_attribute_error_if_pending(_py) {
                    return Ok(None);
                }
                return Err(MoltObject::none().bits());
            }
            return Ok(None);
        };
        let is_list = obj_from_bits(argv_bits)
            .as_ptr()
            .map(|ptr| object_type_id(ptr) == TYPE_ID_LIST)
            .unwrap_or(false);
        if !is_list {
            dec_ref_bits(_py, argv_bits);
            return Ok(None);
        }
        Ok(Some(argv_bits))
    }
}

unsafe fn runpy_begin_sys_argv0_swap(
    _py: &PyToken<'_>,
    argv0_text: &str,
) -> Result<Option<RunpyArgv0SwapState>, u64> {
    unsafe {
        let Some(argv_bits) = runpy_sys_argv_list_bits(_py)? else {
            return Ok(None);
        };
        let Some(argv_ptr) = obj_from_bits(argv_bits).as_ptr() else {
            dec_ref_bits(_py, argv_bits);
            return Ok(None);
        };
        let argv_vec = seq_vec(argv_ptr);
        if argv_vec.is_empty() {
            dec_ref_bits(_py, argv_bits);
            return Ok(None);
        }
        let argv0_ptr = alloc_string(_py, argv0_text.as_bytes());
        if argv0_ptr.is_null() {
            dec_ref_bits(_py, argv_bits);
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        let argv0_bits = MoltObject::from_ptr(argv0_ptr).bits();
        let previous_arg0_bits = argv_vec[0];
        inc_ref_bits(_py, previous_arg0_bits);
        if previous_arg0_bits != argv0_bits {
            inc_ref_bits(_py, argv0_bits);
            argv_vec[0] = argv0_bits;
            dec_ref_bits(_py, previous_arg0_bits);
        }
        dec_ref_bits(_py, argv0_bits);
        dec_ref_bits(_py, argv_bits);
        Ok(Some(RunpyArgv0SwapState { previous_arg0_bits }))
    }
}

unsafe fn runpy_restore_sys_argv0_swap(_py: &PyToken<'_>, state: &mut Option<RunpyArgv0SwapState>) {
    unsafe {
        let Some(state) = state.take() else {
            return;
        };
        let saved_exc_bits = if exception_pending(_py) {
            let bits = molt_exception_last();
            clear_exception(_py);
            Some(bits)
        } else {
            None
        };
        match runpy_sys_argv_list_bits(_py) {
            Ok(Some(argv_bits)) => {
                if let Some(argv_ptr) = obj_from_bits(argv_bits).as_ptr()
                    && object_type_id(argv_ptr) == TYPE_ID_LIST
                {
                    let argv_vec = seq_vec(argv_ptr);
                    if !argv_vec.is_empty() {
                        let current_bits = argv_vec[0];
                        if current_bits != state.previous_arg0_bits {
                            inc_ref_bits(_py, state.previous_arg0_bits);
                            argv_vec[0] = state.previous_arg0_bits;
                            dec_ref_bits(_py, current_bits);
                        }
                    }
                }
                dec_ref_bits(_py, argv_bits);
            }
            Ok(None) => {}
            Err(_) => {}
        }
        dec_ref_bits(_py, state.previous_arg0_bits);
        if let Some(saved_bits) = saved_exc_bits {
            if exception_pending(_py) {
                clear_exception(_py);
            }
            if !obj_from_bits(saved_bits).is_none() {
                let _ = crate::molt_exception_set_last(saved_bits);
            }
            dec_ref_bits(_py, saved_bits);
        }
    }
}

unsafe fn runpy_run_module_from_resolved_source(
    _py: &PyToken<'_>,
    source_path: &str,
    import_name: &str,
    package_name: &str,
    requested_run_name: Option<&str>,
    init_dict_ptr: Option<*mut u8>,
    alter_sys: bool,
) -> u64 {
    let source_bytes = match vfs_read(source_path) {
        Ok(bytes) => bytes,
        Err(err) => {
            let message = err.to_string();
            return match err.kind() {
                std::io::ErrorKind::NotFound => {
                    raise_exception::<_>(_py, "FileNotFoundError", &message)
                }
                std::io::ErrorKind::PermissionDenied => {
                    raise_exception::<_>(_py, "PermissionError", &message)
                }
                std::io::ErrorKind::IsADirectory => {
                    raise_exception::<_>(_py, "IsADirectoryError", &message)
                }
                _ => raise_exception::<_>(_py, "OSError", &message),
            };
        }
    };
    let source = String::from_utf8_lossy(&source_bytes).into_owned();
    let target_name = requested_run_name.unwrap_or(import_name);
    let out_ptr = alloc_dict_with_pairs(_py, &[]);
    if out_ptr.is_null() {
        return raise_exception::<_>(_py, "MemoryError", "out of memory");
    }
    let out_bits = MoltObject::from_ptr(out_ptr).bits();
    let mut alter_sys_modules_state: Option<RunpySysModulesSwapState> = None;
    let mut alter_sys_argv0_state: Option<RunpyArgv0SwapState> = None;
    let exec_result = unsafe {
        (|| -> Result<(), u64> {
            if let Some(init_ptr) = init_dict_ptr {
                dict_copy_entries(_py, init_ptr, out_ptr);
            }
            runpy_apply_source_metadata(
                _py,
                out_ptr,
                target_name,
                import_name,
                source_path,
                package_name,
            )?;
            if alter_sys {
                alter_sys_modules_state = runpy_begin_sys_modules_swap(_py, target_name, out_bits)?;
                alter_sys_argv0_state = runpy_begin_sys_argv0_swap(_py, source_path)?;
            }
            runpy_exec_restricted_source(_py, out_ptr, &source, source_path)?;
            Ok(())
        })()
    };
    unsafe {
        runpy_restore_sys_argv0_swap(_py, &mut alter_sys_argv0_state);
        runpy_restore_sys_modules_swap(_py, &mut alter_sys_modules_state);
    }
    if let Err(err) = exec_result {
        dec_ref_bits(_py, out_bits);
        return err;
    }
    if exception_pending(_py) {
        dec_ref_bits(_py, out_bits);
        return MoltObject::none().bits();
    }
    out_bits
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_runpy_run_module(
    mod_name_bits: u64,
    run_name_bits: u64,
    init_globals_bits: u64,
    alter_sys_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let mod_name = match string_obj_to_owned(obj_from_bits(mod_name_bits)) {
            Some(val) => val,
            None => return raise_exception::<_>(_py, "TypeError", "mod_name must be str"),
        };
        let requested_run_name = {
            let run_name_obj = obj_from_bits(run_name_bits);
            if run_name_obj.is_none() {
                None
            } else {
                match string_obj_to_owned(run_name_obj) {
                    Some(val) => Some(val),
                    None => {
                        return raise_exception::<_>(_py, "TypeError", "run_name must be str");
                    }
                }
            }
        };
        let init_dict_ptr = {
            let init_obj = obj_from_bits(init_globals_bits);
            if init_obj.is_none() {
                None
            } else {
                match init_obj.as_ptr() {
                    Some(ptr) if unsafe { object_type_id(ptr) == TYPE_ID_DICT } => Some(ptr),
                    _ => {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "init_globals must be dict or None",
                        );
                    }
                }
            }
        };
        let alter_sys = is_truthy(_py, obj_from_bits(alter_sys_bits));
        let sys_path = unsafe { runpy_sys_path_entries(_py) };
        if let Some((source_path, import_name, package_name)) =
            runpy_resolve_module_source(&mod_name, &sys_path)
        {
            return unsafe {
                runpy_run_module_from_resolved_source(
                    _py,
                    &source_path,
                    &import_name,
                    &package_name,
                    requested_run_name.as_deref(),
                    init_dict_ptr,
                    alter_sys,
                )
            };
        }
        let mut import_name = mod_name.clone();
        let mut module_bits = match unsafe { runpy_import_module_bits(_py, &import_name) } {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        if obj_from_bits(module_bits).is_none() && !exception_pending(_py) {
            let msg = format!("No module named '{mod_name}'");
            return raise_exception::<_>(_py, "ModuleNotFoundError", &msg);
        }
        if obj_from_bits(module_bits).is_none() {
            return MoltObject::none().bits();
        }
        let payload_is_module_or_dict = obj_from_bits(module_bits)
            .as_ptr()
            .map(|ptr| {
                let ty = unsafe { object_type_id(ptr) };
                ty == TYPE_ID_MODULE || ty == TYPE_ID_DICT
            })
            .unwrap_or(false);
        if !payload_is_module_or_dict {
            if !obj_from_bits(module_bits).is_none() {
                dec_ref_bits(_py, module_bits);
            }
            let sys_path = unsafe { runpy_sys_path_entries(_py) };
            let Some((source_path, import_name, package_name)) =
                runpy_resolve_module_source(&mod_name, &sys_path)
            else {
                let msg = format!("No module named '{mod_name}'");
                return raise_exception::<_>(_py, "ModuleNotFoundError", &msg);
            };
            return unsafe {
                runpy_run_module_from_resolved_source(
                    _py,
                    &source_path,
                    &import_name,
                    &package_name,
                    requested_run_name.as_deref(),
                    init_dict_ptr,
                    alter_sys,
                )
            };
        }
        let mut module_dict_ptr = match unsafe { runpy_module_dict_ptr(_py, module_bits) } {
            Ok(ptr) => ptr,
            Err(bits) => {
                dec_ref_bits(_py, module_bits);
                return bits;
            }
        };
        if unsafe { runpy_module_is_package(_py, module_dict_ptr) } {
            let package_bits = module_bits;
            import_name = format!("{mod_name}.__main__");
            module_bits = match unsafe { runpy_import_module_bits(_py, &import_name) } {
                Ok(bits) => bits,
                Err(bits) => {
                    dec_ref_bits(_py, package_bits);
                    return bits;
                }
            };
            dec_ref_bits(_py, package_bits);
            if obj_from_bits(module_bits).is_none() && !exception_pending(_py) {
                let message = format!(
                    "No module named '{import_name}'; '{mod_name}' is a package and cannot be directly executed"
                );
                return raise_exception::<_>(_py, "ImportError", &message);
            }
            if obj_from_bits(module_bits).is_none() {
                return MoltObject::none().bits();
            }
            module_dict_ptr = match unsafe { runpy_module_dict_ptr(_py, module_bits) } {
                Ok(ptr) => ptr,
                Err(bits) => {
                    dec_ref_bits(_py, module_bits);
                    return bits;
                }
            };
        }
        let target_name = requested_run_name
            .clone()
            .unwrap_or_else(|| import_name.clone());
        let mut alter_sys_modules_state: Option<RunpySysModulesSwapState> = None;
        if alter_sys {
            alter_sys_modules_state =
                match unsafe { runpy_begin_sys_modules_swap(_py, &target_name, module_bits) } {
                    Ok(state) => state,
                    Err(err) => {
                        dec_ref_bits(_py, module_bits);
                        return err;
                    }
                };
        }
        let out_ptr = alloc_dict_with_pairs(_py, &[]);
        if out_ptr.is_null() {
            unsafe {
                runpy_restore_sys_modules_swap(_py, &mut alter_sys_modules_state);
            }
            dec_ref_bits(_py, module_bits);
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        let out_bits = MoltObject::from_ptr(out_ptr).bits();
        let metadata_result: Result<(), u64> = unsafe {
            dict_copy_entries(_py, module_dict_ptr, out_ptr);
            if let Some(init_ptr) = init_dict_ptr {
                dict_copy_entries(_py, init_ptr, out_ptr);
            }
            runpy_apply_module_metadata(_py, module_dict_ptr, out_ptr, &target_name)
        };
        unsafe {
            runpy_restore_sys_modules_swap(_py, &mut alter_sys_modules_state);
        }
        if let Err(err) = metadata_result {
            dec_ref_bits(_py, out_bits);
            dec_ref_bits(_py, module_bits);
            return err;
        }
        if exception_pending(_py) {
            dec_ref_bits(_py, out_bits);
            dec_ref_bits(_py, module_bits);
            return MoltObject::none().bits();
        }
        dec_ref_bits(_py, module_bits);
        out_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_runpy_run_path(
    path_bits: u64,
    run_name_bits: u64,
    init_globals_bits: u64,
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
        let run_name = {
            let run_name_obj = obj_from_bits(run_name_bits);
            if run_name_obj.is_none() {
                "<run_path>".to_string()
            } else {
                match string_obj_to_owned(run_name_obj) {
                    Some(value) => value,
                    None => return raise_exception::<_>(_py, "TypeError", "run_name must be str"),
                }
            }
        };
        let init_dict_ptr = {
            let init_obj = obj_from_bits(init_globals_bits);
            if init_obj.is_none() {
                None
            } else {
                match init_obj.as_ptr() {
                    Some(ptr) if unsafe { object_type_id(ptr) == TYPE_ID_DICT } => Some(ptr),
                    _ => {
                        return raise_exception::<_>(
                            _py,
                            "TypeError",
                            "init_globals must be dict or None",
                        );
                    }
                }
            }
        };
        match std::fs::metadata(&path) {
            Ok(meta) if meta.is_file() => {}
            Ok(_) => return raise_exception::<_>(_py, "FileNotFoundError", &path),
            Err(err) => {
                let message = err.to_string();
                return match err.kind() {
                    std::io::ErrorKind::NotFound => {
                        raise_exception::<_>(_py, "FileNotFoundError", &message)
                    }
                    std::io::ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, "PermissionError", &message)
                    }
                    std::io::ErrorKind::IsADirectory => {
                        raise_exception::<_>(_py, "IsADirectoryError", &message)
                    }
                    _ => raise_exception::<_>(_py, "OSError", &message),
                };
            }
        }
        let source_bytes = match vfs_read(&path) {
            Ok(bytes) => bytes,
            Err(err) => {
                let message = err.to_string();
                return match err.kind() {
                    std::io::ErrorKind::NotFound => {
                        raise_exception::<_>(_py, "FileNotFoundError", &message)
                    }
                    std::io::ErrorKind::PermissionDenied => {
                        raise_exception::<_>(_py, "PermissionError", &message)
                    }
                    std::io::ErrorKind::IsADirectory => {
                        raise_exception::<_>(_py, "IsADirectoryError", &message)
                    }
                    _ => raise_exception::<_>(_py, "OSError", &message),
                };
            }
        };
        let source = String::from_utf8_lossy(&source_bytes).into_owned();
        let out_ptr = alloc_dict_with_pairs(_py, &[]);
        if out_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        let out_bits = MoltObject::from_ptr(out_ptr).bits();
        let mut argv0_swap_state = match unsafe { runpy_begin_sys_argv0_swap(_py, &path) } {
            Ok(state) => state,
            Err(err) => {
                dec_ref_bits(_py, out_bits);
                return err;
            }
        };
        let exec_result = unsafe {
            (|| -> Result<(), u64> {
                if let Some(init_ptr) = init_dict_ptr {
                    dict_copy_entries(_py, init_ptr, out_ptr);
                }

                let run_name_ptr = alloc_string(_py, run_name.as_bytes());
                if run_name_ptr.is_null() {
                    return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
                }
                let run_name_value_bits = MoltObject::from_ptr(run_name_ptr).bits();
                dict_set_str_key_bits(_py, out_ptr, "__name__", run_name_value_bits)?;
                dec_ref_bits(_py, run_name_value_bits);

                let path_ptr = alloc_string(_py, path.as_bytes());
                if path_ptr.is_null() {
                    return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
                }
                let path_value_bits = MoltObject::from_ptr(path_ptr).bits();
                dict_set_str_key_bits(_py, out_ptr, "__file__", path_value_bits)?;
                dec_ref_bits(_py, path_value_bits);

                let package = runpy_package_name(&run_name);
                let package_ptr = alloc_string(_py, package.as_bytes());
                if package_ptr.is_null() {
                    return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
                }
                let package_bits = MoltObject::from_ptr(package_ptr).bits();
                dict_set_str_key_bits(_py, out_ptr, "__package__", package_bits)?;
                dec_ref_bits(_py, package_bits);

                let none_bits = MoltObject::none().bits();
                dict_set_str_key_bits(_py, out_ptr, "__cached__", none_bits)?;
                dict_set_str_key_bits(_py, out_ptr, "__spec__", none_bits)?;
                dict_set_str_key_bits(_py, out_ptr, "__doc__", none_bits)?;
                dict_set_str_key_bits(_py, out_ptr, "__loader__", none_bits)?;

                // NOTE(dynamic-exec-policy): Keep runpy on restricted source execution for
                // compiled binaries. Full code-object execution is intentionally deferred
                // until an explicit capability-gated design, perf evidence, and user
                // approval are in place.
                runpy_exec_restricted_source(_py, out_ptr, &source, &path)?;
                Ok(())
            })()
        };
        unsafe {
            runpy_restore_sys_argv0_swap(_py, &mut argv0_swap_state);
        }
        if let Err(err) = exec_result {
            dec_ref_bits(_py, out_bits);
            return err;
        }
        if exception_pending(_py) {
            dec_ref_bits(_py, out_bits);
            return MoltObject::none().bits();
        }
        out_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_exec_restricted_source(
    namespace_bits: u64,
    source_bits: u64,
    filename_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let namespace_ptr = match obj_from_bits(namespace_bits).as_ptr() {
            Some(ptr) if unsafe { object_type_id(ptr) == TYPE_ID_DICT } => ptr,
            _ => return raise_exception::<_>(_py, "TypeError", "namespace must be dict"),
        };
        let source = match string_obj_to_owned(obj_from_bits(source_bits)) {
            Some(value) => value,
            None => return raise_exception::<_>(_py, "TypeError", "source must be str"),
        };
        let filename = match string_obj_to_owned(obj_from_bits(filename_bits)) {
            Some(value) => value,
            None => return raise_exception::<_>(_py, "TypeError", "filename must be str"),
        };
        // NOTE(dynamic-exec-policy): Keep importlib source execution in the
        // restricted intrinsic lane for compiled binaries. Do not widen to
        // unrestricted code-object execution without an approved capability gate
        // and measured perf impact.
        unsafe {
            if let Err(err) = runpy_exec_restricted_source(_py, namespace_ptr, &source, &filename) {
                return err;
            }
        }
        MoltObject::none().bits()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn restricted_literal_parser_supports_core_values() {
        assert_eq!(
            parse_restricted_literal("None"),
            Some(RestrictedLiteral::NoneValue)
        );
        assert_eq!(
            parse_restricted_literal("True"),
            Some(RestrictedLiteral::Bool(true))
        );
        assert_eq!(
            parse_restricted_literal("-12"),
            Some(RestrictedLiteral::Int(-12))
        );
        assert_eq!(
            parse_restricted_literal("1.25"),
            Some(RestrictedLiteral::Float(1.25))
        );
        assert_eq!(
            parse_restricted_literal("'hello\\nworld'"),
            Some(RestrictedLiteral::Str("hello\nworld".to_string()))
        );
        assert_eq!(
            parse_restricted_literal("b'abc'"),
            Some(RestrictedLiteral::Bytes(b"abc".to_vec()))
        );
        assert_eq!(
            parse_restricted_literal("[1, 2, 3]"),
            Some(RestrictedLiteral::List(vec![
                RestrictedLiteral::Int(1),
                RestrictedLiteral::Int(2),
                RestrictedLiteral::Int(3),
            ]))
        );
        assert_eq!(
            parse_restricted_literal("(1, 'x')"),
            Some(RestrictedLiteral::Tuple(vec![
                RestrictedLiteral::Int(1),
                RestrictedLiteral::Str("x".to_string()),
            ]))
        );
        assert_eq!(
            parse_restricted_literal("{'a': 1, 'b': [2, 3]}"),
            Some(RestrictedLiteral::Dict(vec![
                (
                    RestrictedLiteral::Str("a".to_string()),
                    RestrictedLiteral::Int(1),
                ),
                (
                    RestrictedLiteral::Str("b".to_string()),
                    RestrictedLiteral::List(vec![
                        RestrictedLiteral::Int(2),
                        RestrictedLiteral::Int(3),
                    ]),
                ),
            ]))
        );
    }

    #[test]
    fn identifier_parser_matches_basic_python_rules() {
        assert!(is_identifier_text("_value"));
        assert!(is_identifier_text("alpha9"));
        assert!(is_identifier_text("Δx"));
        assert!(!is_identifier_text("9abc"));
        assert!(!is_identifier_text("a-b"));
        assert!(is_dotted_identifier_text("pkg.mod"));
        assert!(!is_dotted_identifier_text("pkg..mod"));
    }

    #[test]
    fn strip_inline_comment_preserves_strings() {
        assert_eq!(strip_inline_comment_text("x = 1 # tail"), "x = 1");
        assert_eq!(strip_inline_comment_text("x = 'a#b'  # tail"), "x = 'a#b'");
        assert_eq!(
            strip_inline_comment_text("x = \"a#b\\\"c\" # tail"),
            "x = \"a#b\\\"c\""
        );
    }

    #[test]
    fn restricted_reference_parser_accepts_attr_and_literal_indices() {
        assert_eq!(
            parse_restricted_reference_expr("sys.argv[0]"),
            Some(RestrictedReferenceExpr {
                base: "sys".to_string(),
                steps: vec![
                    RestrictedReferenceStep::Attr("argv".to_string()),
                    RestrictedReferenceStep::Index(RestrictedReferenceIndex::Int(0)),
                ],
            })
        );
        assert_eq!(
            parse_restricted_reference_expr("cfg.paths['root'][1]"),
            Some(RestrictedReferenceExpr {
                base: "cfg".to_string(),
                steps: vec![
                    RestrictedReferenceStep::Attr("paths".to_string()),
                    RestrictedReferenceStep::Index(RestrictedReferenceIndex::Str(
                        "root".to_string()
                    )),
                    RestrictedReferenceStep::Index(RestrictedReferenceIndex::Int(1)),
                ],
            })
        );
        assert_eq!(
            parse_restricted_reference_expr("value"),
            Some(RestrictedReferenceExpr {
                base: "value".to_string(),
                steps: vec![],
            })
        );
    }

    #[test]
    fn restricted_reference_parser_rejects_dynamic_rhs_forms() {
        assert_eq!(parse_restricted_reference_expr("a[b]"), None);
        assert_eq!(parse_restricted_reference_expr("items[1:3]"), None);
        assert_eq!(parse_restricted_reference_expr("loader()"), None);
        assert_eq!(parse_restricted_reference_expr("sys.argv[0]()"), None);
        assert_eq!(parse_restricted_reference_expr("a + b"), None);
        assert_eq!(parse_restricted_reference_expr("sys.argv[True]"), None);
    }

    #[test]
    fn restricted_source_rejects_function_definition_without_partial_success() {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let namespace_ptr = alloc_dict_with_pairs(_py, &[]);
                assert!(!namespace_ptr.is_null());
                let namespace_bits = MoltObject::from_ptr(namespace_ptr).bits();
                let source = "allowed = 1\n\ndef unsupported():\n    return allowed\n";

                let result = runpy_exec_restricted_source(_py, namespace_ptr, source, "dynamic.py");

                assert!(
                    result.is_err(),
                    "unsupported function body must fail closed"
                );
                assert!(
                    exception_pending(_py),
                    "unsupported function body must leave an exception for callers"
                );
                let (kind, message) = pending_import_exception_kind_and_message(_py)
                    .expect("pending restricted-source exception");
                assert_eq!(kind, "NotImplementedError");
                assert!(message.contains("unsupported module statement"));
                let unsupported_ptr = alloc_string(_py, b"unsupported");
                assert!(!unsupported_ptr.is_null());
                let unsupported_name = MoltObject::from_ptr(unsupported_ptr).bits();
                assert!(
                    dict_get_in_place(_py, namespace_ptr, unsupported_name).is_none(),
                    "unsupported function definitions must not be materialized"
                );
                dec_ref_bits(_py, unsupported_name);
                clear_exception(_py);
                dec_ref_bits(_py, namespace_bits);
            }
        });
    }

    #[test]
    fn restricted_reference_eval_supports_module_attr_then_subscript() {
        crate::with_gil_entry_nopanic!(_py, {
            unsafe {
                let namespace_ptr = alloc_dict_with_pairs(_py, &[]);
                assert!(!namespace_ptr.is_null());
                let namespace_bits = MoltObject::from_ptr(namespace_ptr).bits();

                let name_ptr = alloc_string(_py, b"sys");
                assert!(!name_ptr.is_null());
                let name_bits = MoltObject::from_ptr(name_ptr).bits();
                let module_ptr = alloc_module_obj(_py, name_bits);
                assert!(!module_ptr.is_null());
                dec_ref_bits(_py, name_bits);
                let module_bits = MoltObject::from_ptr(module_ptr).bits();

                let argv0_ptr = alloc_string(_py, b"/tmp/runpy_test.py");
                assert!(!argv0_ptr.is_null());
                let argv0_bits = MoltObject::from_ptr(argv0_ptr).bits();
                let argv_ptr = alloc_list(_py, &[argv0_bits]);
                assert!(!argv_ptr.is_null());
                dec_ref_bits(_py, argv0_bits);
                let argv_bits = MoltObject::from_ptr(argv_ptr).bits();

                let module_dict = module_dict_bits(module_ptr);
                let module_dict_ptr = obj_from_bits(module_dict)
                    .as_ptr()
                    .expect("module dict ptr");
                assert_eq!(object_type_id(module_dict_ptr), TYPE_ID_DICT);
                dict_set_str_key_bits(_py, module_dict_ptr, "argv", argv_bits).expect("set argv");
                dec_ref_bits(_py, argv_bits);

                dict_set_str_key_bits(_py, namespace_ptr, "sys", module_bits).expect("set sys");
                dec_ref_bits(_py, module_bits);

                let expr = parse_restricted_reference_expr("sys.argv[0]").expect("reference parse");
                let value_bits =
                    runpy_eval_restricted_reference_expr(_py, namespace_ptr, &expr).expect("eval");
                let value_text = string_obj_to_owned(obj_from_bits(value_bits));
                assert_eq!(value_text.as_deref(), Some("/tmp/runpy_test.py"));
                dec_ref_bits(_py, value_bits);

                dec_ref_bits(_py, namespace_bits);
            }
        });
    }

    #[test]
    fn runpy_package_name_uses_parent_module() {
        assert_eq!(runpy_package_name("pkg.tool"), "pkg");
        assert_eq!(runpy_package_name("single"), "");
    }
}
