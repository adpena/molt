use super::*;
use crate::builtins::compatibility_error::CompatibilityError;
use digest::Digest;
use sha1::Sha1;

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_bootstrap_path(module_file_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let state = sys_bootstrap_state_from_module_file(_py, module_file);
        alloc_string_list_bits(_py, &state.path).unwrap_or_else(|| MoltObject::none().bits())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_bootstrap_pythonpath(module_file_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let state = sys_bootstrap_state_from_module_file(_py, module_file);
        alloc_string_list_bits(_py, &state.pythonpath_entries)
            .unwrap_or_else(|| MoltObject::none().bits())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_bootstrap_module_roots(module_file_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let state = sys_bootstrap_state_from_module_file(_py, module_file);
        alloc_string_list_bits(_py, &state.module_roots_entries)
            .unwrap_or_else(|| MoltObject::none().bits())
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_bootstrap_pwd(module_file_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let state = sys_bootstrap_state_from_module_file(_py, module_file);
        match alloc_str_bits(_py, &state.pwd) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_bootstrap_include_cwd(module_file_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let state = sys_bootstrap_state_from_module_file(_py, module_file);
        MoltObject::from_bool(state.include_cwd).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_sys_bootstrap_stdlib_root(module_file_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let state = sys_bootstrap_state_from_module_file(_py, module_file);
        match state.stdlib_root {
            Some(root) => match alloc_str_bits(_py, &root) {
                Ok(bits) => bits,
                Err(err) => err,
            },
            None => MoltObject::none().bits(),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_source_loader_payload(
    module_name_bits: u64,
    path_bits: u64,
    spec_is_package_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let spec_is_package = is_truthy(_py, obj_from_bits(spec_is_package_bits));
        let resolution = source_loader_resolution(&module_name, &path, spec_is_package);
        match importlib_loader_resolution_payload_bits(_py, &resolution) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

/// Record the extension-admission role on the actual class declared by
/// machinery. Names, facade rebinding and sys.modules removal cannot erase it.
/// Declaring this role only adds admission checks; it never authorizes loading.
#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_extension_loader_type_declare(class_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(ptr) = obj_from_bits(class_bits).as_ptr() else {
            return raise_exception::<_>(
                py,
                "TypeError",
                "extension loader declaration requires a type",
            );
        };
        if unsafe { object_type_id(ptr) } != TYPE_ID_TYPE {
            return raise_exception::<_>(
                py,
                "TypeError",
                "extension loader declaration requires a type",
            );
        }
        unsafe {
            crate::object::class_storage::class_declare(
                ptr,
                crate::object::class_storage::ClassDeclaration::ExtensionLoader,
            );
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_extension_loader_payload(
    module_name_bits: u64,
    path_bits: u64,
    spec_is_package_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let spec_is_package = is_truthy(_py, obj_from_bits(spec_is_package_bits));
        let resolution = match importlib_extension_loader_resolution_checked(
            _py,
            &module_name,
            &path,
            spec_is_package,
        ) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match importlib_loader_resolution_payload_bits(_py, &resolution) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

pub(super) fn importlib_zip_source_exec_payload_checked(
    _py: &PyToken<'_>,
    module_name: &str,
    archive_path: &str,
    inner_path: &str,
    spec_is_package: bool,
) -> Result<ImportlibZipSourceExecPayload, u64> {
    let allowed = has_capability(_py, "fs.read");
    audit_capability_decision(
        "importlib.zip.source_exec_payload",
        "fs.read",
        AuditArgs::None,
        allowed,
    );
    if !allowed {
        return Err(raise_exception::<_>(
            _py,
            "PermissionError",
            "missing fs.read capability",
        ));
    }
    importlib_zip_source_exec_payload(module_name, archive_path, inner_path, spec_is_package)
        .map_err(|err| raise_importlib_io_error(_py, err))
}

pub(super) fn importlib_extension_loader_resolution_checked(
    _py: &PyToken<'_>,
    module_name: &str,
    path: &str,
    spec_is_package: bool,
) -> Result<SourceLoaderResolution, u64> {
    let allowed = has_capability(_py, "fs.read");
    audit_capability_decision(
        "importlib.extension_loader_payload",
        "fs.read",
        AuditArgs::None,
        allowed,
    );
    if !allowed {
        return Err(raise_exception::<_>(
            _py,
            "PermissionError",
            "missing fs.read capability",
        ));
    }
    match importlib_path_is_file(_py, path) {
        Ok(true) => {}
        Ok(false) => {
            return Err(raise_exception::<_>(
                _py,
                "ImportError",
                "extension module path must point to a file",
            ));
        }
        Err(bits) => return Err(bits),
    }
    importlib_require_extension_metadata(_py, module_name, path)?;
    Ok(extension_loader_resolution(
        module_name,
        path,
        spec_is_package,
    ))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_zip_source_exec_payload(
    module_name_bits: u64,
    archive_path_bits: u64,
    inner_path_bits: u64,
    spec_is_package_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let archive_path = match string_arg_from_bits(_py, archive_path_bits, "archive path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let inner_path = match string_arg_from_bits(_py, inner_path_bits, "inner path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let spec_is_package = is_truthy(_py, obj_from_bits(spec_is_package_bits));
        let payload = match importlib_zip_source_exec_payload_checked(
            _py,
            &module_name,
            &archive_path,
            &inner_path,
            spec_is_package,
        ) {
            Ok(value) => value,
            Err(bits) => return bits,
        };

        let source_ptr = alloc_string(_py, &payload.source);
        if source_ptr.is_null() {
            return crate::builtins::exceptions::ExceptionSentinel::exception_sentinel();
        }
        let source_bits = MoltObject::from_ptr(source_ptr).bits();
        let origin_bits = match alloc_str_bits(_py, &payload.origin) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, source_bits);
                return err;
            }
        };
        let module_package_bits = match alloc_str_bits(_py, &payload.module_package) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, source_bits);
                dec_ref_bits(_py, origin_bits);
                return err;
            }
        };
        let package_root_bits = match payload.package_root.as_deref() {
            Some(root) => match alloc_str_bits(_py, root) {
                Ok(bits) => bits,
                Err(err) => {
                    dec_ref_bits(_py, source_bits);
                    dec_ref_bits(_py, origin_bits);
                    dec_ref_bits(_py, module_package_bits);
                    return err;
                }
            },
            None => MoltObject::none().bits(),
        };
        let is_package_bits = MoltObject::from_bool(payload.is_package).bits();

        let keys_and_values: [(&[u8], u64); 5] = [
            (b"source", source_bits),
            (b"origin", origin_bits),
            (b"is_package", is_package_bits),
            (b"module_package", module_package_bits),
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

fn importlib_admit_dynamic_extension(
    _py: &PyToken<'_>,
    module_name: &str,
    path: &str,
) -> Result<Option<SourceExtensionLoaderContract>, u64> {
    crate::require_operation::<u64>(
        _py,
        crate::OperationId::ImportlibExecExtensionModule,
        AuditArgs::None,
    )?;
    match importlib_path_is_file(_py, path) {
        Ok(true) => {}
        Ok(false) => {
            return Err(raise_exception::<_>(
                _py,
                "ImportError",
                "extension module path must point to a file",
            ));
        }
        Err(bits) => return Err(bits),
    }
    importlib_require_extension_metadata(_py, module_name, path)?;
    importlib_source_extension_loader_contract(_py, module_name, path)
}

#[cfg(all(feature = "cext_loader", not(target_arch = "wasm32")))]
fn importlib_extension_exec_owns_publication(
    _py: &PyToken<'_>,
    module_bits: u64,
    module_name_bits: u64,
) -> Result<bool, u64> {
    if !importlib_spec_exec_owns(module_bits) {
        return Ok(false);
    }
    let modules_bits = importlib_runtime_modules_bits(_py)?;
    let visible = match obj_from_bits(modules_bits).as_ptr() {
        Some(modules_ptr) => importlib_dict_get_string_key_bits(_py, modules_ptr, module_name_bits),
        None => Err(importlib_modules_runtime_error(_py)),
    };
    dec_ref_bits(_py, modules_bits);
    if visible? != Some(module_bits) {
        return Err(raise_exception::<_>(
            _py,
            "ImportError",
            "extension exec module differs from the import transaction's sys.modules entry",
        ));
    }
    Ok(true)
}

pub(in crate::builtins::platform) fn importlib_exec_extension_impl(
    _py: &PyToken<'_>,
    module_bits: u64,
    _module_name_bits: u64,
    module_name: &str,
    path: &str,
) -> Result<(), u64> {
    // Compiler-admitted application modules are independent of their source
    // or extension-shaped origin. The immutable build catalog is the sole
    // body authority; only a catalog miss enters the genuine dynamic loader.
    if importlib_try_exec_compiled_module(_py, module_bits, module_name)? {
        return Ok(());
    }
    if let Some(contract) = importlib_admit_dynamic_extension(_py, module_name, path)? {
        #[cfg(all(feature = "source_extension_loader", not(target_arch = "wasm32")))]
        {
            let namespace_ptr = importlib_module_dict_ptr_for_state(_py, module_bits)?;
            match source_extension_loader_dlopen(
                _py,
                namespace_ptr,
                module_name,
                path,
                &contract.init_symbol,
            ) {
                Ok(()) => return Ok(()),
                Err(msg) => {
                    return Err(raise_exception::<_>(
                        _py,
                        "ImportError",
                        &format!(
                            "failed to load libmolt source extension {module_name:?} from {path:?}: {msg}"
                        ),
                    ));
                }
            }
        }
        #[cfg(not(all(feature = "source_extension_loader", not(target_arch = "wasm32"))))]
        {
            return Err(raise_exception::<_>(
                _py,
                "ImportError",
                &format!(
                    "extension {module_name:?} at {path:?} requires source_extension_loader runtime support for init symbol {:?}",
                    contract.init_symbol
                ),
            ));
        }
    }
    // -- Native C extension loading via dlopen --
    #[cfg(all(feature = "cext_loader", not(target_arch = "wasm32")))]
    {
        let publish_cache =
            importlib_extension_exec_owns_publication(_py, module_bits, _module_name_bits)?;
        let result = crate::cpython_abi_hooks::execute_prepared_extension(
            module_bits,
            _module_name_bits,
            publish_cache,
        );
        if exception_pending(_py) {
            if !obj_from_bits(result).is_none() {
                dec_ref_bits(_py, result);
            }
            return Err(MoltObject::none().bits());
        }
        if !obj_from_bits(result).is_none() {
            dec_ref_bits(_py, result);
            return Err(raise_exception::<_>(
                _py,
                "SystemError",
                "C extension execution returned a non-None result",
            ));
        }
        return Ok(());
    }
    #[cfg(not(all(feature = "cext_loader", not(target_arch = "wasm32"))))]
    Err(importlib_extension_exec_unavailable(
        _py,
        module_name,
        path,
        "extension",
    ))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_extension_loader_create_module(
    module_name_bits: u64,
    path_bits: u64,
    _spec_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        // A compiler-admitted extension-shaped module has its own in-place
        // execution contract; creating a second native module would split it.
        if crate::builtins::module_table::module_execution_target_has_body(&module_name)
            == Some(true)
        {
            return MoltObject::none().bits();
        }
        match importlib_admit_dynamic_extension(_py, &module_name, &path) {
            Ok(Some(_)) => return MoltObject::none().bits(),
            Ok(None) => {}
            Err(bits) => return bits,
        }
        #[cfg(all(feature = "cext_loader", not(target_arch = "wasm32")))]
        {
            molt_cpython_abi::bridge::molt_cpython_abi_init();
            if !crate::cpython_abi_hooks::register_cpython_hooks() {
                return raise_exception::<_>(
                    _py,
                    "ImportError",
                    "process-static C extensions require the primary runtime",
                );
            }
            return match unsafe {
                molt_cpython_abi::loader::create_cpython_extension(
                    std::path::Path::new(&path),
                    &module_name,
                    _spec_bits,
                )
            } {
                Ok(bits) => bits,
                Err(error) => {
                    if crate::cpython_abi_hooks::transfer_pending_cpython_exception()
                        || exception_pending(_py)
                    {
                        MoltObject::none().bits()
                    } else {
                        raise_exception::<_>(
                            _py,
                            "ImportError",
                            &format!(
                                "failed to create C extension {module_name:?} from {path:?}: {error}"
                            ),
                        )
                    }
                }
            };
        }
        #[cfg(not(all(feature = "cext_loader", not(target_arch = "wasm32"))))]
        importlib_extension_exec_unavailable(_py, &module_name, &path, "extension")
    })
}

pub(super) struct ImportlibLoaderExecContext {
    module_name_bits: u64,
    module_name: String,
}

/// The body a file loader's `exec_module` runs. As CPython's loaders do, it
/// only executes the body: `module_from_spec` is the one site that runs
/// `_init_module_attrs`, so execution sets no module metadata.
pub(super) enum ImportlibLoaderExecBody<'a> {
    CompiledSource,
    Extension { path: &'a str },
}

pub(super) fn importlib_loader_exec_context(
    _py: &PyToken<'_>,
    module_bits: u64,
    loader_bits: u64,
) -> Result<ImportlibLoaderExecContext, u64> {
    let module_name_bits = importlib_coerce_module_name_bits(
        _py,
        module_bits,
        loader_bits,
        MoltObject::none().bits(),
    )?;
    let Some(module_name) = string_obj_to_owned(obj_from_bits(module_name_bits)) else {
        if !obj_from_bits(module_name_bits).is_none() {
            dec_ref_bits(_py, module_name_bits);
        }
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "module name must be str",
        ));
    };
    Ok(ImportlibLoaderExecContext {
        module_name_bits,
        module_name,
    })
}

pub(super) fn importlib_loader_exec_context_release(
    _py: &PyToken<'_>,
    ctx: &ImportlibLoaderExecContext,
) {
    if !obj_from_bits(ctx.module_name_bits).is_none() {
        dec_ref_bits(_py, ctx.module_name_bits);
    }
}

pub(super) fn importlib_loader_exec_module_apply(
    _py: &PyToken<'_>,
    module_bits: u64,
    ctx: &ImportlibLoaderExecContext,
    body: ImportlibLoaderExecBody<'_>,
) -> Result<(), u64> {
    // The target must be a module with a dictionary namespace.
    importlib_module_dict_ptr_for_state(_py, module_bits)?;
    match body {
        ImportlibLoaderExecBody::CompiledSource => {
            importlib_exec_compiled_module(_py, module_bits, &ctx.module_name)
        }
        ImportlibLoaderExecBody::Extension { path } => importlib_exec_extension_impl(
            _py,
            module_bits,
            ctx.module_name_bits,
            &ctx.module_name,
            path,
        ),
    }
}

/// Run one loader body for `module` under its coerced name.
fn importlib_loader_exec_module(
    _py: &PyToken<'_>,
    loader_bits: u64,
    module_bits: u64,
    body: impl FnOnce(&ImportlibLoaderExecContext) -> Result<(), u64>,
) -> u64 {
    let ctx = match importlib_loader_exec_context(_py, module_bits, loader_bits) {
        Ok(value) => value,
        Err(bits) => return bits,
    };
    let out = body(&ctx);
    importlib_loader_exec_context_release(_py, &ctx);
    match out {
        Ok(()) => MoltObject::none().bits(),
        Err(bits) => bits,
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_sourcefileloader_exec_module(
    loader_bits: u64,
    module_bits: u64,
    path_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        if let Err(bits) = string_arg_from_bits(_py, path_bits, "path") {
            return bits;
        }
        importlib_loader_exec_module(_py, loader_bits, module_bits, |ctx| {
            importlib_loader_exec_module_apply(
                _py,
                module_bits,
                ctx,
                ImportlibLoaderExecBody::CompiledSource,
            )
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_zip_source_loader_exec_module(
    loader_bits: u64,
    module_bits: u64,
    archive_path_bits: u64,
    inner_path_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let archive_path = match string_arg_from_bits(_py, archive_path_bits, "archive path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let inner_path = match string_arg_from_bits(_py, inner_path_bits, "inner path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        importlib_loader_exec_module(_py, loader_bits, module_bits, |ctx| {
            // zipimporter.exec_module reads the archive member before running
            // it; the read admits the capability and the member's presence.
            let spec_is_package = importlib_module_spec_is_package_bits(_py, module_bits)?;
            importlib_zip_source_exec_payload_checked(
                _py,
                &ctx.module_name,
                &archive_path,
                &inner_path,
                spec_is_package,
            )?;
            importlib_loader_exec_module_apply(
                _py,
                module_bits,
                ctx,
                ImportlibLoaderExecBody::CompiledSource,
            )
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_extension_loader_exec_module(
    loader_bits: u64,
    module_bits: u64,
    path_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        importlib_loader_exec_module(_py, loader_bits, module_bits, |ctx| {
            importlib_loader_exec_module_apply(
                _py,
                module_bits,
                ctx,
                ImportlibLoaderExecBody::Extension { path: &path },
            )
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_sourceless_loader_exec_module(
    loader_bits: u64,
    module_bits: u64,
    path_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if importlib_is_archive_member_path(&path) {
            return raise_exception::<_>(_py, "NotADirectoryError", &path);
        }
        // The compiled body stands in for the bytecode this loader would run.
        importlib_loader_exec_module(_py, loader_bits, module_bits, |ctx| {
            importlib_loader_exec_module_apply(
                _py,
                module_bits,
                ctx,
                ImportlibLoaderExecBody::CompiledSource,
            )
        })
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_linecache_loader_get_source(loader_bits: u64, module_name_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match linecache_loader_get_source_impl(_py, loader_bits, &module_name) {
            Ok(Some(source)) => match alloc_str_bits(_py, &source) {
                Ok(bits) => bits,
                Err(err) => err,
            },
            Ok(None) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

pub(super) fn importlib_coerce_module_name_bits(
    _py: &PyToken<'_>,
    module_bits: u64,
    loader_bits: u64,
    spec_bits: u64,
) -> Result<u64, u64> {
    let module_name_name = intern_runtime_static_name(_py, b"__name__");
    let publish_name = |name_bits| {
        let result = crate::molt_set_attr_name(module_bits, module_name_name, name_bits);
        dec_ref_bits(_py, result);
        if exception_pending(_py) {
            dec_ref_bits(_py, name_bits);
            Err(MoltObject::none().bits())
        } else {
            Ok(name_bits)
        }
    };
    if let Some(module_name_bits) = getattr_optional_bits(_py, module_bits, module_name_name)? {
        if string_obj_to_owned(obj_from_bits(module_name_bits)).is_some() {
            return Ok(module_name_bits);
        }
        if !obj_from_bits(module_name_bits).is_none() {
            dec_ref_bits(_py, module_name_bits);
        }
    }

    let mut module_spec_bits = spec_bits;
    let mut module_spec_owned = false;
    if obj_from_bits(module_spec_bits).is_none() {
        let spec_name = intern_runtime_static_name(_py, b"__spec__");
        if let Some(bits) = getattr_optional_bits(_py, module_bits, spec_name)? {
            module_spec_bits = bits;
            module_spec_owned = true;
        }
    }

    let name_attribute = intern_runtime_static_name(_py, b"name");
    if !obj_from_bits(module_spec_bits).is_none()
        && let Some(spec_name_bits) =
            match getattr_optional_bits(_py, module_spec_bits, name_attribute) {
                Ok(value) => value,
                Err(bits) => {
                    if module_spec_owned {
                        dec_ref_bits(_py, module_spec_bits);
                    }
                    return Err(bits);
                }
            }
    {
        if string_obj_to_owned(obj_from_bits(spec_name_bits)).is_some() {
            if module_spec_owned {
                dec_ref_bits(_py, module_spec_bits);
            }
            return publish_name(spec_name_bits);
        }
        if !obj_from_bits(spec_name_bits).is_none() {
            dec_ref_bits(_py, spec_name_bits);
        }
    }

    if module_spec_owned && !obj_from_bits(module_spec_bits).is_none() {
        dec_ref_bits(_py, module_spec_bits);
    }

    if let Some(loader_name_bits) = getattr_optional_bits(_py, loader_bits, name_attribute)? {
        if string_obj_to_owned(obj_from_bits(loader_name_bits)).is_some() {
            return publish_name(loader_name_bits);
        }
        if !obj_from_bits(loader_name_bits).is_none() {
            dec_ref_bits(_py, loader_name_bits);
        }
    }

    Err(raise_exception::<_>(
        _py,
        "TypeError",
        "module name must be str",
    ))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_coerce_module_name(
    module_bits: u64,
    loader_bits: u64,
    spec_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match importlib_coerce_module_name_bits(_py, module_bits, loader_bits, spec_bits) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_package_root_from_origin(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match importlib_package_root_from_origin(&path) {
            Some(root) => match alloc_str_bits(_py, &root) {
                Ok(bits) => bits,
                Err(err) => err,
            },
            None => MoltObject::none().bits(),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_traceback_exception_suppress_context(value_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        match traceback_exception_suppress_context_bits(_py, value_bits) {
            Ok(value) => MoltObject::from_bool(value).bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_zip_archive_path(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let path = match string_arg_from_bits(py, path_bits, "archive path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if let Err(error) = crate::require_operation::<u64>(
            py,
            crate::OperationId::ImportlibZipArchivePath,
            AuditArgs::None,
        ) {
            return error;
        }
        let Some((archive, prefix)) = split_zip_archive_path(&path) else {
            return MoltObject::none().bits();
        };
        // Use the same central-directory parser as native discovery. A regular
        // file with an archive suffix is not enough to claim a path hook.
        #[cfg(feature = "stdlib_archive")]
        let admitted = zip_archive_open(&archive).is_ok();
        #[cfg(not(feature = "stdlib_archive"))]
        let admitted = false;
        if !admitted {
            return MoltObject::none().bits();
        }
        let archive_bits = match alloc_str_bits(py, &archive) {
            Ok(bits) => bits,
            Err(bits) => return bits,
        };
        let prefix_bits = match alloc_str_bits(py, &prefix) {
            Ok(bits) => bits,
            Err(bits) => {
                dec_ref_bits(py, archive_bits);
                return bits;
            }
        };
        let result = alloc_tuple(py, &[archive_bits, prefix_bits]);
        dec_ref_bits(py, archive_bits);
        dec_ref_bits(py, prefix_bits);
        if result.is_null() {
            crate::builtins::exceptions::ExceptionSentinel::exception_sentinel()
        } else {
            MoltObject::from_ptr(result).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_zip_read_entry(
    archive_path_bits: u64,
    inner_path_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision(
            "importlib.zip.read_entry",
            "fs.read",
            AuditArgs::None,
            allowed,
        );
        if !allowed {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let archive_path = match string_arg_from_bits(_py, archive_path_bits, "archive path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let inner_path = match string_arg_from_bits(_py, inner_path_bits, "inner path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let normalized = inner_path.replace('\\', "/").trim_matches('/').to_string();
        if normalized.is_empty() {
            return raise_exception::<_>(_py, "OSError", "zip archive entry path is empty");
        }
        let bytes = match zip_archive_read_entry(&archive_path, &normalized) {
            Ok(value) => value,
            Err(err) => return raise_importlib_io_error(_py, err),
        };
        let out_ptr = alloc_bytes(_py, &bytes);
        if out_ptr.is_null() {
            crate::builtins::exceptions::ExceptionSentinel::exception_sentinel()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_read_file(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let allowed = has_capability(_py, "fs.read");
        audit_capability_decision("importlib.read_file", "fs.read", AuditArgs::None, allowed);
        if !allowed {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let bytes = match importlib_read_file_bytes(_py, &path) {
            Ok(bytes) => bytes,
            Err(bits) => return bits,
        };
        let out_ptr = alloc_bytes(_py, &bytes);
        if out_ptr.is_null() {
            crate::builtins::exceptions::ExceptionSentinel::exception_sentinel()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_cache_from_source(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        use crate::builtins::exceptions::ExceptionValue;
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(sys) = crate::builtins::modules::interpreter_sys_module(_py) else {
            return raise_exception::<_>(_py, "RuntimeError", "sys module is unavailable");
        };
        inc_ref_bits(_py, sys);
        let sys = ExceptionValue::adopt(_py, sys);
        let read = |object, name: &[u8]| -> Option<ExceptionValue<'_, '_>> {
            let key = attr_name_bits_from_bytes(_py, name)?;
            let key = ExceptionValue::adopt(_py, key);
            crate::builtins::attributes::get_attr_name(_py, object, key.bits())
                .ok()
                .map(|bits| ExceptionValue::adopt(_py, bits))
        };
        let Some(implementation) = read(sys.bits(), b"implementation") else {
            return MoltObject::none().bits();
        };
        let Some(tag) = read(implementation.bits(), b"cache_tag") else {
            return MoltObject::none().bits();
        };
        if obj_from_bits(tag.bits()).is_none() {
            return CompatibilityError::ImportlibCacheTagUnavailable { is_none: true }.raise(_py);
        }
        let tag = match string_arg_from_bits(_py, tag.bits(), "cache_tag") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(suffixes) =
            crate::builtins::types::module_spec_suffix_value(_py, b"BYTECODE_SUFFIXES")
        else {
            return MoltObject::none().bits();
        };
        let suffixes = ExceptionValue::adopt(_py, suffixes);
        let suffix = crate::molt_index(suffixes.bits(), MoltObject::from_int(0).bits());
        let suffix = ExceptionValue::adopt(_py, suffix);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let suffix = match string_arg_from_bits(_py, suffix.bits(), "bytecode suffix") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let cached = importlib_cache_from_source(&path, &tag, &suffix);
        match alloc_str_bits(_py, &cached) {
            Ok(bits) => bits,
            Err(err_bits) => err_bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_decode_source(source_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let Some(source_ptr) = obj_from_bits(source_bits).as_ptr() else {
            inc_ref_bits(_py, source_bits);
            return source_bits;
        };
        let source_type = unsafe { object_type_id(source_ptr) };
        if source_type != TYPE_ID_BYTES && source_type != TYPE_ID_BYTEARRAY {
            inc_ref_bits(_py, source_bits);
            return source_bits;
        }
        let encoding_bits = match alloc_str_bits(_py, "utf-8") {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let decoded_bits = if source_type == TYPE_ID_BYTES {
            molt_bytes_decode(source_bits, encoding_bits, MoltObject::none().bits())
        } else {
            molt_bytearray_decode(source_bits, encoding_bits, MoltObject::none().bits())
        };
        dec_ref_bits(_py, encoding_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        decoded_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_source_hash(source_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let source = match bytes_arg_from_bits(_py, source_bits, "source_bytes") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut hasher = Sha1::new();
        hasher.update(&source);
        let digest = hasher.finalize();
        let out_ptr = alloc_bytes(_py, &digest[..8]);
        if out_ptr.is_null() {
            crate::builtins::exceptions::ExceptionSentinel::exception_sentinel()
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_source_from_cache(path_bits: u64) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        inc_ref_bits(_py, path_bits);
        path_bits
    })
}
