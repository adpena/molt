use super::*;
use crate::BuiltinExc;

/// The Python facade owns reload discovery/reentry; this is only _exec.
#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_reload(module_bits: u64, spec_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let name = match string_arg_from_bits(py, name_bits, "name") {
            Ok(name) => name,
            Err(error) => return error,
        };
        let dictionary = match importlib_runtime_modules_bits(py) {
            Ok(bits) => bits,
            Err(error) => return error,
        };
        let result = importlib_spec_execution_transaction(
            py,
            &name,
            name_bits,
            spec_bits,
            crate::ptr_from_bits(dictionary),
            ImportlibSpecExecutionOptions {
                target: ImportlibSpecTarget::Exact(module_bits),
                allow_load_module_fallback: true,
            },
        );
        molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, dictionary));
        result.unwrap_or_else(|error| error)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_bootstrap_payload(
    search_paths_bits: u64,
    module_file_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let search_paths =
            match string_sequence_arg_from_bits(_py, search_paths_bits, "search paths") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let payload = importlib_bootstrap_payload(_py, &search_paths, module_file);
        let resolved_paths_bits = match alloc_string_list_bits(_py, &payload.resolved_search_paths)
        {
            Some(bits) => bits,
            None => return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel(),
        };
        let pythonpath_bits = match alloc_string_list_bits(_py, &payload.pythonpath_entries) {
            Some(bits) => bits,
            None => {
                dec_ref_bits(_py, resolved_paths_bits);
                return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
            }
        };
        let module_roots_bits = match alloc_string_list_bits(_py, &payload.module_roots_entries) {
            Some(bits) => bits,
            None => {
                dec_ref_bits(_py, resolved_paths_bits);
                dec_ref_bits(_py, pythonpath_bits);
                return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
            }
        };
        let venv_site_packages_bits =
            match alloc_string_list_bits(_py, &payload.venv_site_packages_entries) {
                Some(bits) => bits,
                None => {
                    dec_ref_bits(_py, resolved_paths_bits);
                    dec_ref_bits(_py, pythonpath_bits);
                    dec_ref_bits(_py, module_roots_bits);
                    return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
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
pub extern "C" fn molt_importlib_search_paths(
    search_paths_bits: u64,
    module_file_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let search_paths =
            match string_sequence_arg_from_bits(_py, search_paths_bits, "search paths") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let resolved = importlib_search_paths(_py, &search_paths, module_file);
        match alloc_string_list_bits(_py, &resolved) {
            Some(bits) => bits,
            None => crate::builtins::exceptions::ExceptionSentinel::exception_sentinel(),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_namespace_paths(
    package_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.namespace_paths",
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
        let resolved = importlib_namespace_paths(_py, &package, &search_paths, module_file);
        match alloc_string_list_bits(_py, &resolved) {
            Some(bits) => bits,
            None => crate::builtins::exceptions::ExceptionSentinel::exception_sentinel(),
        }
    })
}

fn import_state_retired(py: &PyToken<'_>) -> bool {
    platform_state(py)
        .import_retired
        .load(std::sync::atomic::Ordering::Acquire)
}

/// Shutdown's terminal disposition, for tests that exercise the refusal.
#[cfg(test)]
pub(in super::super) fn importlib_retire_import_state(py: &PyToken<'_>) {
    platform_state(py)
        .import_retired
        .store(true, std::sync::atomic::Ordering::Release);
}

pub(in super::super) fn importlib_acquire_import_lock(py: &PyToken<'_>) -> Result<(), u64> {
    if import_state_retired(py) {
        return Err(raise_exception::<_>(
            py,
            BuiltinExc::ImportError,
            "interpreter import state is retired",
        ));
    }
    let lock = &platform_state(py).import_lock;
    if !lock.try_acquire() {
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _release = GilReleaseGuard::suspend();
            lock.acquire(None);
        }
        #[cfg(target_arch = "wasm32")]
        unreachable!("single-threaded reentrant import lock");
    }
    if import_state_retired(py) {
        lock.release();
        return Err(raise_exception::<_>(
            py,
            BuiltinExc::ImportError,
            "interpreter import state is retired",
        ));
    }
    Ok(())
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_imp_acquire_lock() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        importlib_acquire_import_lock(py).map_or_else(|error| error, |()| MoltObject::none().bits())
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_imp_release_lock() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        if !platform_state(py).import_lock.release() {
            return raise_exception::<_>(
                py,
                BuiltinExc::RuntimeError,
                "not holding the import lock",
            );
        }
        MoltObject::none().bits()
    })
}
#[unsafe(no_mangle)]
pub extern "C" fn molt_imp_lock_held() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        MoltObject::from_bool(platform_state(py).import_lock.locked()).bits()
    })
}
