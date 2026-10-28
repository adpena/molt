use super::*;

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
        let resolved = importlib_namespace_paths(_py, &package, &search_paths, module_file);
        match alloc_string_list_bits(_py, &resolved) {
            Some(bits) => bits,
            None => crate::builtins::exceptions::ExceptionSentinel::exception_sentinel(),
        }
    })
}

pub(in super::super) fn pending_default_owners(
    state: DefaultFinderGeneration,
) -> Option<PendingDefaultFinders> {
    match state {
        DefaultFinderGeneration::Pending(pending) | DefaultFinderGeneration::Loading(pending) => {
            Some(pending)
        }
        _ => None,
    }
}

pub(in super::super) fn release_default_owners(
    py: &PyToken<'_>,
    pending: Option<PendingDefaultFinders>,
) {
    if let Some(pending) = pending {
        dec_ref_bits(py, pending.meta);
        dec_ref_bits(py, pending.hooks);
    }
}

pub(crate) fn importlib_seed_default_generation(py: &PyToken<'_>, meta: u64, hooks: u64) {
    inc_ref_bits(py, meta);
    inc_ref_bits(py, hooks);
    let epoch = |bits| unsafe {
        let backing = crate::object::layout::seq_vec_ptr(ptr_from_bits(bits));
        crate::object::backing::tracked_vec_mutation_epoch(backing)
    };
    let pending = PendingDefaultFinders {
        meta,
        meta_epoch: epoch(meta),
        hooks,
        hooks_epoch: epoch(hooks),
    };
    let previous = {
        let mut state = platform_state(py).import_defaults.lock().unwrap();
        if matches!(*state, DefaultFinderGeneration::Retired) {
            drop(state);
            release_default_owners(py, Some(pending));
            return;
        }
        std::mem::replace(&mut *state, DefaultFinderGeneration::Pending(pending))
    };
    release_default_owners(py, pending_default_owners(previous));
}

pub(in super::super) fn importlib_take_default_generation(
    state: &crate::state::RuntimeState,
) -> Option<PendingDefaultFinders> {
    let previous = {
        let mut defaults = state.platform.import_defaults.lock().unwrap();
        std::mem::replace(&mut *defaults, DefaultFinderGeneration::Retired)
    };
    pending_default_owners(previous)
}

#[cfg(test)]
pub(in super::super) fn importlib_retire_default_generation(
    py: &PyToken<'_>,
    state: &crate::state::RuntimeState,
) -> bool {
    let owners = importlib_take_default_generation(state);
    let changed = owners.is_some();
    release_default_owners(py, owners);
    changed
}

pub(in super::super) fn importlib_acquire_import_lock(py: &PyToken<'_>) -> Result<(), u64> {
    if matches!(
        *platform_state(py).import_defaults.lock().unwrap(),
        DefaultFinderGeneration::Retired
    ) {
        return Err(raise_exception::<_>(
            py,
            "ImportError",
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
    if matches!(
        *platform_state(py).import_defaults.lock().unwrap(),
        DefaultFinderGeneration::Retired
    ) {
        lock.release();
        return Err(raise_exception::<_>(
            py,
            "ImportError",
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
            return raise_exception::<_>(py, "RuntimeError", "not holding the import lock");
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

/// The finite source capability, not a mutable module spelling, admits the
/// filesystem facade. Microbinaries without runtime-import support add no body.
fn initial_machinery_capability() -> Option<&'static str> {
    crate::builtins::module_table::module_registry()?
        .compiled_spec("importlib.machinery")
        .filter(|spec| spec.is_stdlib_provider)
        .map(|spec| spec.provider_name)
}

pub(in super::super) fn importlib_complete_initial_defaults(py: &PyToken<'_>) -> Result<(), u64> {
    if !matches!(
        *platform_state(py).import_defaults.lock().unwrap(),
        DefaultFinderGeneration::Pending(_)
    ) {
        return if matches!(
            *platform_state(py).import_defaults.lock().unwrap(),
            DefaultFinderGeneration::Failed
        ) {
            Err(raise_exception::<_>(
                py,
                "ImportError",
                "initial import finders failed to initialize",
            ))
        } else {
            Ok(())
        };
    }
    let Some(provider) = initial_machinery_capability() else {
        let previous = {
            let mut state = platform_state(py).import_defaults.lock().unwrap();
            match *state {
                DefaultFinderGeneration::Pending(_) => {
                    std::mem::replace(&mut *state, DefaultFinderGeneration::Cancelled)
                }
                _ => DefaultFinderGeneration::Cancelled,
            }
        };
        release_default_owners(py, pending_default_owners(previous));
        return Ok(());
    };
    // Use the existing all-name execution owner. A foreign caller waits for the
    // actual body; same-thread bootstrap recursion may use the early builtin.
    let _execution = crate::builtins::module_table::ModuleExecutionGuard::acquire(py, provider)
        .map_err(|failure| failure.into_exception(py, provider))?;
    {
        let mut state = platform_state(py).import_defaults.lock().unwrap();
        match *state {
            DefaultFinderGeneration::Pending(pending) => {
                *state = DefaultFinderGeneration::Loading(pending)
            }
            _ => return Ok(()),
        }
    }
    // A public observation may become the last owner during a body callback.
    // Retire it only after success or failure publishes a terminal disposition.
    let mut publication_owner = None;
    let result = (|| {
        let publication = crate::builtins::modules::public_module_cache_lookup(py, provider)?;
        publication_owner = match &publication {
            crate::builtins::modules::PublicModuleCache::Present(bits) => Some(
                crate::builtins::exceptions::ExceptionValue::adopt(py, *bits),
            ),
            _ => None,
        };
        if matches!(
            publication,
            crate::builtins::modules::PublicModuleCache::Missing
        ) {
            // Normal startup publishes its real namespace once. A cache hit is
            // only public membership, never evidence that this body executed.
            let key = crate::builtins::exceptions::ExceptionValue::adopt(
                py,
                alloc_str_bits(py, provider)?,
            );
            match crate::builtins::modules::catalog_import_attempt(key.bits())? {
                crate::builtins::modules::ModuleImportOutcome::Imported(bits) => {
                    let _module = crate::builtins::exceptions::ExceptionValue::adopt(py, bits);
                }
                crate::builtins::modules::ModuleImportOutcome::Missing { .. } => {
                    return Err(raise_exception::<u64>(
                        py,
                        "ImportError",
                        "admitted machinery initializer is unavailable",
                    ));
                }
            }
        }
        let needs_body = matches!(
            *platform_state(py).import_defaults.lock().unwrap(),
            DefaultFinderGeneration::Loading(_)
        );
        if needs_body {
            // An existing/reentrant replacement cannot supply bootstrap facts.
            // Execute the same admitted body in its private namespace without
            // changing any public entry or parent attribute.
            let module = execute_compiled_module(
                py,
                provider,
                provider,
                None,
                false,
                ExecutionMetadata::Module { argv0: None },
            )
            .map_err(|error| error.into_import_error(py, provider))?;
            let _module = crate::builtins::exceptions::ExceptionValue::adopt(py, module);
        }
        let completed = matches!(
            *platform_state(py).import_defaults.lock().unwrap(),
            DefaultFinderGeneration::Complete | DefaultFinderGeneration::Cancelled
        );
        if completed {
            Ok(())
        } else {
            Err(raise_exception::<u64>(
                py,
                "RuntimeError",
                "admitted machinery body did not complete initial finders",
            ))
        }
    })();
    if result.is_err() {
        let previous = {
            let mut state = platform_state(py).import_defaults.lock().unwrap();
            match *state {
                DefaultFinderGeneration::Pending(_) | DefaultFinderGeneration::Loading(_) => {
                    std::mem::replace(&mut *state, DefaultFinderGeneration::Failed)
                }
                _ => DefaultFinderGeneration::Failed,
            }
        };
        molt_cpython_abi::api::errors::with_preserved_error(|| {
            release_default_owners(py, pending_default_owners(previous))
        });
    }
    molt_cpython_abi::api::errors::with_preserved_error(|| drop(publication_owner));
    result
}

pub(in super::super) fn importlib_finish_initial_defaults(
    py: &PyToken<'_>,
    machinery: u64,
) -> Result<(), u64> {
    use crate::builtins::exceptions::ExceptionValue;
    let pending = {
        let state = platform_state(py).import_defaults.lock().unwrap();
        let pending = match *state {
            DefaultFinderGeneration::Pending(pending)
            | DefaultFinderGeneration::Loading(pending) => pending,
            _ => return Ok(()),
        };
        inc_ref_bits(py, pending.meta);
        inc_ref_bits(py, pending.hooks);
        pending
    };
    let _meta_owner = ExceptionValue::adopt(py, pending.meta);
    let _hooks_owner = ExceptionValue::adopt(py, pending.hooks);
    let result = (|| -> Result<bool, u64> {
        let sys = importlib_system_module(py)?;
        let read = |object, key| -> Result<ExceptionValue<'_, '_>, u64> {
            let bits = crate::builtins::attributes::get_attr_name(py, object, key)
                .map_err(|()| MoltObject::none().bits())?;
            Ok(ExceptionValue::adopt(py, bits))
        };
        let meta_name = intern_runtime_static_name(py, b"meta_path");
        let hooks_name = intern_runtime_static_name(py, b"path_hooks");
        if meta_name == 0 || hooks_name == 0 {
            return Err(MoltObject::none().bits());
        }
        let meta = read(sys.bits(), meta_name)?;
        let hooks = read(sys.bits(), hooks_name)?;
        let own_meta = meta.bits() == pending.meta;
        let own_hooks = hooks.bits() == pending.hooks;
        if !own_meta && !own_hooks {
            return Ok(false);
        }
        let path_finder = ExceptionValue::adopt(
            py,
            importlib_machinery_member(
                py,
                machinery,
                runtime_static_name_slot(py, b"PathFinder"),
                b"PathFinder",
            )?,
        );
        let defaults_name = attr_name_bits_from_bytes(py, b"_default_path_hooks")
            .ok_or_else(|| MoltObject::none().bits())?;
        let defaults_name = ExceptionValue::adopt(py, defaults_name);
        let defaults = crate::object::ops::molt_getitem_builtin(machinery, defaults_name.bits());
        let defaults = ExceptionValue::adopt(py, defaults);
        if exception_pending(py) {
            return Err(MoltObject::none().bits());
        }
        let defaults_ptr = obj_from_bits(defaults.bits())
            .as_ptr()
            .filter(|ptr| unsafe { object_type_id(*ptr) == TYPE_ID_LIST })
            .ok_or_else(|| {
                raise_exception::<u64>(py, "TypeError", "default path hooks must be a list")
            })?;
        let defaults = unsafe {
            crate::object::seq_access::snapshot(
                py,
                defaults_ptr,
                "default path hooks snapshot allocation failed",
            )
        }
        .ok_or_else(|| MoltObject::none().bits())?;
        let active = {
            let state = platform_state(py).import_defaults.lock().unwrap();
            matches!(*state, DefaultFinderGeneration::Pending(current) | DefaultFinderGeneration::Loading(current)
                if current.meta == pending.meta && current.meta_epoch == pending.meta_epoch
                    && current.hooks == pending.hooks && current.hooks_epoch == pending.hooks_epoch)
        };
        if !active {
            return Ok(false);
        }
        // Each append validates its own original generation. A customized hook
        // list is not repaired; a customized finder list never gets PathFinder.
        if own_hooks {
            unsafe {
                crate::object::list_mutation::extend_if_epoch(
                    py,
                    ptr_from_bits(pending.hooks),
                    pending.hooks_epoch,
                    &defaults,
                )
            }
            .map_err(|()| MoltObject::none().bits())?;
        }
        if !own_meta {
            return Ok(false);
        }
        let meta = unsafe {
            crate::object::list_mutation::extend_if_epoch(
                py,
                ptr_from_bits(pending.meta),
                pending.meta_epoch,
                &[path_finder.bits()],
            )
        }
        .map_err(|()| MoltObject::none().bits())?;
        Ok(meta.is_some())
    })();
    let next = match result {
        Ok(true) => DefaultFinderGeneration::Complete,
        Ok(false) => DefaultFinderGeneration::Cancelled,
        Err(_) => DefaultFinderGeneration::Failed,
    };
    let previous = {
        let mut state = platform_state(py).import_defaults.lock().unwrap();
        // Shutdown/customization may have retired this exact admission during
        // an attribute callback. Never resurrect it or overwrite another cohort.
        match *state {
            DefaultFinderGeneration::Pending(current)
            | DefaultFinderGeneration::Loading(current)
                if current.meta == pending.meta && current.meta_epoch == pending.meta_epoch =>
            {
                std::mem::replace(&mut *state, next)
            }
            _ => return result.map(|_| ()),
        }
    };
    molt_cpython_abi::api::errors::with_preserved_error(|| {
        release_default_owners(py, pending_default_owners(previous))
    });
    result.map(|_| ())
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_ensure_default_meta_path(machinery: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(provider) = initial_machinery_capability() else {
            return raise_exception::<_>(
                py,
                "RuntimeError",
                "default finder producer has no admitted source body",
            );
        };
        if !crate::builtins::modules::module_body_owns_globals(py, provider, machinery) {
            return raise_exception::<_>(
                py,
                "RuntimeError",
                "default finder producer does not own its defining globals",
            );
        }
        importlib_finish_initial_defaults(py, machinery)
            .map_or_else(|error| error, |()| MoltObject::none().bits())
    })
}
