//! One owned PyInit transaction for static native/WASM and explicit dynamic loading.
//! The runtime import cache owns publication; ABI views remain stable while C
//! objects retain them. Failure unwinds cache/state/C owners with errors detached.

use super::*;

thread_local! {
    // Rust-owned names carry no GC roots. All linked header transports use
    // the same runtime-owned context and restore it across nested imports.
    static PACKAGE_CONTEXT: std::cell::RefCell<Option<String>> = const {
        std::cell::RefCell::new(None)
    };
}

struct PackageContext(Option<String>);

struct LegacySnapshotOwner(u64);

impl Drop for LegacySnapshotOwner {
    fn drop(&mut self) {
        with_preserved_error(|| unsafe { hook_dec_ref(self.0) });
    }
}

impl PackageContext {
    fn enter(name: String) -> Self {
        Self(PACKAGE_CONTEXT.with(|slot| slot.replace(Some(name))))
    }
}

impl Drop for PackageContext {
    fn drop(&mut self) {
        PACKAGE_CONTEXT.with(|slot| slot.replace(self.0.take()));
    }
}

pub(super) unsafe extern "C" fn hook_alloc_extension_module(data: *const u8, len: usize) -> u64 {
    let name = unsafe { std::slice::from_raw_parts(data, len) };
    let qualified = PACKAGE_CONTEXT.with(|slot| {
        let mut context = slot.borrow_mut();
        if context
            .as_ref()
            .and_then(|name| name.rsplit_once('.'))
            .is_some_and(|(_, leaf)| leaf.as_bytes() == name)
        {
            context.take()
        } else {
            None
        }
    });
    let name = qualified.as_deref().map(str::as_bytes).unwrap_or(name);
    unsafe { hook_alloc_module(name.as_ptr(), name.len()) }
}

unsafe fn run_extension_init(
    py: &crate::PyToken<'_>,
    init: unsafe extern "C" fn() -> *mut PyObject,
    name_bits: u64,
    origin_bits: u64,
    spec_bits: u64,
    create_only: bool,
) -> u64 {
    let Some(name) = crate::string_obj_to_owned(MoltObject::from_bits(name_bits)) else {
        return crate::raise_exception::<u64>(
            py,
            "TypeError",
            "extension module name must be a str",
        );
    };
    if name.is_empty() {
        return crate::raise_exception::<u64>(
            py,
            "ValueError",
            "extension module name must not be empty",
        );
    }
    let identity = crate::c_api::ExtensionImportIdentity {
        init_address: init as usize,
        name: name.clone(),
        origin: crate::string_obj_to_owned(MoltObject::from_bits(origin_bits)),
    };
    let previous = crate::c_api::module_extension_import(py, &identity);
    if let Some((def, Some(snapshot))) = previous {
        let _snapshot_owner = LegacySnapshotOwner(snapshot);
        let result = restore_legacy_extension(
            py,
            def as *mut PyModuleDef,
            snapshot,
            name_bits,
            &name,
            origin_bits,
            spec_bits,
        );
        return result;
    }
    let result = {
        let _context = PackageContext::enter(name);
        unsafe { init() }
    };
    // This boundary is indivisible to generated-code exception checks. A C
    // result with an error is always validated and released, never abandoned.
    initialize_result(
        py,
        result,
        ExtensionInitialization {
            name_bits,
            origin_bits,
            supplied_spec: spec_bits,
            create_only,
            import_identity: Some(&identity),
            reload: previous.is_some(),
        },
    )
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_cpython_abi_run_static_extension_init(
    init_address: u64,
    name_bits: u64,
) -> u64 {
    with_gil(|py| {
        if !register_cpython_hooks() {
            if crate::exception_pending(&py) {
                return MoltObject::none().bits();
            }
            return crate::raise_exception::<u64>(
                &py,
                "SystemError",
                "C extension runtime hook registration failed",
            );
        }
        let Ok(address) = usize::try_from(init_address) else {
            return crate::raise_exception::<u64>(
                &py,
                "SystemError",
                "extension initializer address is out of range",
            );
        };
        if address == 0 {
            return crate::raise_exception::<u64>(
                &py,
                "SystemError",
                "extension initializer is NULL",
            );
        }
        let init = unsafe {
            std::mem::transmute::<usize, unsafe extern "C" fn() -> *mut PyObject>(address)
        };
        unsafe {
            run_extension_init(
                &py,
                init,
                name_bits,
                MoltObject::none().bits(),
                MoltObject::none().bits(),
                false,
            )
        }
    })
}

/// Creation executes and publishes single-phase modules. Only multi-phase
/// create_module defers publication/execution to its caller. Legacy replay has
/// already admitted its public identity through import_add_module.
#[derive(Clone, Copy)]
enum ExtensionPhase {
    SinglePhase { reload: bool },
    LegacyReplay,
    MultiPhase { execute: bool },
}

/// Public admission is independent of private-cache publication. Multi-phase
/// execution cannot replace a different public module; single-phase init may.
/// Every admitted result still replaces a stale private entry with its identity.
#[derive(Clone, Copy, Eq, PartialEq)]
enum ExtensionPublicationAdmission {
    Replace,
    VacantOrSelf,
}

struct ModulePublication {
    object: *mut PyObject,
    name_bits: u64,
    def: *mut PyModuleDef,
    register_state: bool,
    published: bool,
    committed: bool,
}

impl ModulePublication {
    fn publish(
        &mut self,
        py: &crate::PyToken<'_>,
        name: &str,
        bits: u64,
        admission: ExtensionPublicationAdmission,
    ) -> Result<(), String> {
        if admission == ExtensionPublicationAdmission::VacantOrSelf {
            let existing = crate::builtins::modules::molt_module_cache_get(self.name_bits);
            if crate::exception_pending(py) {
                return Err(String::new());
            }
            let conflicts = existing != bits && !MoltObject::from_bits(existing).is_none();
            dec_ref_bits(py, existing);
            if conflicts {
                return Err(format!(
                    "{name}: extension publication found a different module"
                ));
            }
        }
        // Publication can fail after installing only some owners. Rollback
        // detaches our exact identity independently from all three stores.
        self.published = true;
        let _ = crate::builtins::modules::module_cache_publish(
            self.name_bits,
            bits,
            crate::builtins::modules::ModuleCachePublication::Extension,
        );
        if crate::exception_pending(py) {
            return Err(String::new());
        }
        Ok(())
    }

    fn register(
        &mut self,
        bits: u64,
        import: Option<(&crate::c_api::ExtensionImportIdentity, Option<u64>)>,
    ) -> Result<(), String> {
        if self.register_state {
            if crate::c_api::module_state_add_with_import(bits, self.def.addr(), import) != 0 {
                return Err("extension module state registration failed".into());
            }
            self.register_state = false;
        }
        Ok(())
    }

    unsafe fn commit(mut self) -> Result<u64, String> {
        // Acquire an independent runtime owner, then Drop releases only the
        // constructor C ref without dismantling a projection still referenced
        // by native m_self edges.
        let Some(bits) =
            (unsafe { molt_cpython_abi::bridge::GLOBAL_BRIDGE.molt_value_for_pyobj(self.object) })
        else {
            return Err("extension module result could not transfer ABI ownership".into());
        };
        debug_assert!(!self.register_state);
        self.committed = true;
        Ok(bits)
    }
}

impl Drop for ModulePublication {
    fn drop(&mut self) {
        with_preserved_error(|| {
            if !self.committed
                && self.published
                && let Some(own) = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                    .molt_handle_for_pyobj(self.object)
                    .map(|value| value.bits())
            {
                let _ = crate::builtins::modules::module_cache_remove(self.name_bits, Some(own));
            }
            unsafe { molt_cpython_abi::api::refcount::Py_DECREF(self.object) };
        });
    }
}

struct ModuleResultContext<'a> {
    name_bits: u64,
    name: &'a str,
    spec: *mut PyObject,
    origin_bits: u64,
    phase: ExtensionPhase,
    import_identity: Option<&'a crate::c_api::ExtensionImportIdentity>,
}

unsafe fn module_result_to_bits(
    py: &crate::PyToken<'_>,
    object: *mut PyObject,
    def: *mut PyModuleDef,
    context: ModuleResultContext<'_>,
) -> Result<u64, String> {
    let ModuleResultContext {
        name_bits,
        name,
        spec,
        origin_bits,
        phase,
        import_identity,
    } = context;
    let single_phase = !matches!(phase, ExtensionPhase::MultiPhase { .. });
    let mut publication = ModulePublication {
        object,
        def,
        register_state: single_phase,
        name_bits,
        published: false,
        committed: false,
    };
    let Some(bits) = molt_cpython_abi::bridge::GLOBAL_BRIDGE
        .molt_handle_for_pyobj(object)
        .map(|value| value.bits())
    else {
        return Err(format!(
            "{name}: extension PyInit returned an invalid module handle"
        ));
    };
    let Some(ptr) = MoltObject::from_bits(bits).as_ptr() else {
        return Err(format!(
            "{name}: extension PyInit returned a non-module object"
        ));
    };
    if unsafe { object_type_id(ptr) } != TYPE_ID_MODULE {
        return Err(format!(
            "{name}: extension PyInit returned a non-module object"
        ));
    }
    if publication.def.is_null() {
        publication.def = crate::c_api::molt_module_capi_get_def(bits) as *mut PyModuleDef;
    }
    if single_phase && publication.def.is_null() {
        return Err(format!("{name}: PyInit did not return an extension module"));
    }
    let spec_bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
        .molt_handle_for_pyobj(spec)
        .expect("runtime-owned spec has a managed view")
        .bits();
    if !unsafe {
        module_set_bits(bits, b"__spec__", spec_bits)
            && (MoltObject::from_bits(origin_bits).is_none()
                || module_set_bits(bits, b"__file__", origin_bits))
    } {
        return Err(format!(
            "{name}: extension module metadata initialization failed"
        ));
    }
    // Package specs own their own name as parent. Splitting the module name
    // here would create a second, incorrect metadata authority for packages.
    for (source, destination) in [(c"loader", c"__loader__"), (c"parent", c"__package__")] {
        let value =
            unsafe { molt_cpython_abi::api::object::PyObject_GetAttrString(spec, source.as_ptr()) };
        if value.is_null() {
            return Err(format!(
                "{name}: extension spec has no {}",
                source.to_string_lossy()
            ));
        }
        let status = unsafe {
            molt_cpython_abi::api::object::PyObject_SetAttrString(
                object,
                destination.as_ptr(),
                value,
            )
        };
        with_preserved_error(|| unsafe { molt_cpython_abi::api::refcount::Py_DECREF(value) });
        if status != 0 {
            return Err(format!(
                "{name}: extension {} metadata initialization failed",
                destination.to_string_lossy()
            ));
        }
    }
    // The first successful single-phase import registers before publication
    // on 3.13+, but 3.12 and repeatable reloads on every supported version
    // publish first. Retired public owners can execute callbacks here.
    let register_first = matches!(phase, ExtensionPhase::SinglePhase { reload: false })
        && crate::object::ops_sys::runtime_target_at_least(py, 3, 13);
    let register = |publication: &mut ModulePublication| -> Result<(), String> {
        let snapshot = if matches!(phase, ExtensionPhase::SinglePhase { .. })
            && import_identity.is_some()
            && unsafe { (*publication.def).m_size == -1 }
        {
            let dict = unsafe { crate::object::layout::module_dict_bits(ptr) };
            let copied = crate::molt_dict_copy(dict);
            if crate::exception_pending(py) {
                return Err(String::new());
            }
            Some(LegacySnapshotOwner(copied))
        } else {
            None
        };
        publication.register(
            bits,
            import_identity.map(|identity| (identity, snapshot.as_ref().map(|owner| owner.0))),
        )
    };
    if register_first {
        register(&mut publication)?;
    }
    match phase {
        ExtensionPhase::SinglePhase { .. } => {
            publication.publish(py, name, bits, ExtensionPublicationAdmission::Replace)?;
        }
        ExtensionPhase::MultiPhase { execute: true } => {
            publication.publish(py, name, bits, ExtensionPublicationAdmission::VacantOrSelf)?;
            if !def.is_null()
                && unsafe { molt_cpython_abi::api::modules::PyModule_ExecDef(object, def) } != 0
            {
                return Err(format!(
                    "{name}: PyModuleDef Py_mod_exec slot returned non-zero"
                ));
            }
        }
        ExtensionPhase::MultiPhase { execute: false } | ExtensionPhase::LegacyReplay => {}
    }
    if !register_first {
        register(&mut publication)?;
    }
    unsafe { publication.commit() }
}

unsafe fn module_spec(
    py: &crate::PyToken<'_>,
    name_bits: u64,
    origin_bits: u64,
) -> Option<*mut PyObject> {
    let none = MoltObject::none().bits();
    let spec_bits =
        crate::builtins::types::alloc_module_spec(py, name_bits, none, origin_bits, none)?;
    let result = unsafe { cext_owned_pyobject_from_bits(spec_bits) };
    (!result.is_null()).then_some(result)
}

fn extension_spec(
    py: &crate::PyToken<'_>,
    name_bits: u64,
    origin_bits: u64,
    supplied_spec: u64,
) -> Option<*mut PyObject> {
    if MoltObject::from_bits(supplied_spec).is_none() {
        unsafe { module_spec(py, name_bits, origin_bits) }
    } else {
        let ptr = unsafe {
            molt_cpython_abi::bridge::GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(supplied_spec)
        };
        (!ptr.is_null()).then_some(ptr)
    }
}

fn restore_legacy_extension(
    py: &crate::PyToken<'_>,
    def: *mut PyModuleDef,
    snapshot: u64,
    name_bits: u64,
    name: &str,
    origin_bits: u64,
    supplied_spec: u64,
) -> u64 {
    // Legacy replay uses CPython's import_add_module, including during
    // create_module. Existing module identity
    // wins; otherwise a raw new module is published before merging m_copy.
    let bits = match import_add_module_owned(py, name_bits, |bits| {
        crate::builtins::modules::reconcile_extension_publication(py, name, bits);
    }) {
        Ok(bits) => bits,
        Err(_) => return MoltObject::none().bits(),
    };
    let Some(module_ptr) = MoltObject::from_bits(bits).as_ptr() else {
        return initialization_failure(py, "legacy extension module allocation failed");
    };
    let object = unsafe { cext_owned_pyobject_from_bits(bits) };
    if object.is_null() {
        return initialization_failure(py, "legacy extension ABI view allocation failed");
    }
    let dict = unsafe { crate::object::layout::module_dict_bits(module_ptr) };
    let _ = crate::molt_dict_update(dict, snapshot);
    if crate::exception_pending(py) {
        with_preserved_error(|| unsafe { molt_cpython_abi::api::refcount::Py_DECREF(object) });
        return MoltObject::none().bits();
    }
    // A new/unannotated snapshot clone has no physical extension definition;
    // an existing C module keeps its original metadata. Copied native methods
    // intentionally retain the original m_self from the snapshot.
    crate::c_api::module_mark_legacy_reimport(py, bits);
    let Some(spec) = extension_spec(py, name_bits, origin_bits, supplied_spec) else {
        with_preserved_error(|| unsafe { molt_cpython_abi::api::refcount::Py_DECREF(object) });
        return initialization_failure(py, "legacy extension ModuleSpec allocation failed");
    };
    let result = unsafe {
        // import_add_module already owns public publication, including for
        // create_module. Do not reject or republish that same module here.
        // Failure must not remove a preexisting sys.modules entry.
        module_result_to_bits(
            py,
            object,
            def,
            ModuleResultContext {
                name_bits,
                name,
                spec,
                origin_bits,
                phase: ExtensionPhase::LegacyReplay,
                import_identity: None,
            },
        )
    };
    with_preserved_error(|| unsafe { molt_cpython_abi::api::refcount::Py_DECREF(spec) });
    match result {
        Ok(bits) => bits,
        Err(message) => initialization_failure(py, &message),
    }
}

unsafe fn module_set_bits(module: u64, attr: &[u8], value: u64) -> bool {
    unsafe { hook_module_set_attr(module, attr.as_ptr(), attr.len(), value) == 0 }
}

fn initialization_failure(_py: &crate::PyToken<'_>, message: &str) -> u64 {
    let _ = transfer_pending_cpython_exception();
    if crate::exception_pending(_py) {
        // PyInit/Py_mod_create/Py_mod_exec errors are Python exceptions, not
        // loader diagnostics. Preserve their type, identity and traceback.
        return MoltObject::none().bits();
    }
    crate::raise_exception::<u64>(_py, "ImportError", message)
}

fn initialization_contract_violation(_py: &crate::PyToken<'_>, message: &str) -> u64 {
    let _ = transfer_pending_cpython_exception();
    let prior_bits = crate::builtins::exceptions::molt_exception_last_pending();
    let Some(prior_ptr) = crate::obj_from_bits(prior_bits).as_ptr() else {
        return crate::raise_exception::<u64>(_py, "SystemError", message);
    };
    let detail = crate::format_exception_message(_py, prior_ptr);
    let combined = if detail.is_empty() || message.contains(&detail) {
        message.to_owned()
    } else {
        format!("{message}: {detail}")
    };

    // A non-NULL result with an error set violates the C API. CPython reports
    // SystemError while retaining the original exception as its context.
    crate::clear_exception(_py);
    let wrapper_ptr = crate::builtins::exceptions::alloc_exception(_py, "SystemError", &combined);
    if wrapper_ptr.is_null() {
        crate::dec_ref_bits(_py, prior_bits);
        return MoltObject::none().bits();
    }
    let wrapper_bits = MoltObject::from_ptr(wrapper_ptr).bits();
    if crate::builtins::exceptions::exception_replace_field_bits(
        _py,
        wrapper_bits,
        crate::builtins::exceptions::ExceptionFieldSlot::Context,
        prior_bits,
    )
    .is_err()
    {
        crate::dec_ref_bits(_py, wrapper_bits);
        crate::dec_ref_bits(_py, prior_bits);
        return MoltObject::none().bits();
    }
    crate::builtins::exceptions::record_exception_owned(_py, wrapper_ptr);
    crate::dec_ref_bits(_py, prior_bits);
    MoltObject::none().bits()
}

struct ExtensionInitialization<'a> {
    name_bits: u64,
    origin_bits: u64,
    supplied_spec: u64,
    create_only: bool,
    import_identity: Option<&'a crate::c_api::ExtensionImportIdentity>,
    reload: bool,
}

fn initialize_result(
    py: &crate::PyToken<'_>,
    result: *mut PyObject,
    initialization: ExtensionInitialization<'_>,
) -> u64 {
    let ExtensionInitialization {
        name_bits,
        origin_bits,
        supplied_spec,
        create_only,
        import_identity,
        reload,
    } = initialization;
    let has_error = cpython_error_is_pending();
    if result.is_null() {
        if has_error {
            return initialization_failure(py, "extension PyInit returned NULL");
        }
        return crate::raise_exception::<u64>(
            py,
            "SystemError",
            "extension PyInit returned NULL without setting an exception",
        );
    }
    // A mapped runtime object must never be inspected as a larger PyModuleDef.
    let mapped = molt_cpython_abi::bridge::GLOBAL_BRIDGE
        .molt_handle_for_pyobj(result)
        .is_some();
    // PyModuleDef_Init establishes the canonical ABI type in the same runtime
    // image. A name match or plausible trailing bytes is not layout authority.
    let definition =
        !mapped && unsafe { std::ptr::eq((*result).ob_type, &raw const PyModuleDef_Type) };
    if has_error {
        let invalid_definition = definition
            && unsafe {
                (*(result.cast::<PyModuleDef>())).m_name.is_null()
                    || CStr::from_ptr((*(result.cast::<PyModuleDef>())).m_name)
                        .to_bytes()
                        .is_empty()
            };
        if !definition {
            with_preserved_error(|| unsafe { molt_cpython_abi::api::refcount::Py_DECREF(result) });
        }
        return initialization_contract_violation(
            py,
            if invalid_definition {
                "extension PyInit returned an invalid module definition"
            } else {
                "extension PyInit returned a result with an exception set"
            },
        );
    }
    let name = crate::string_obj_to_owned(crate::obj_from_bits(name_bits));
    if name.as_ref().is_none_or(|name| name.is_empty()) {
        if !definition {
            unsafe { molt_cpython_abi::api::refcount::Py_DECREF(result) };
        }
        return crate::raise_exception::<u64>(
            py,
            if name.is_none() {
                "TypeError"
            } else {
                "ValueError"
            },
            "extension module name must be a nonempty str",
        );
    }
    let name = name.unwrap();
    if !definition {
        let valid_module = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(result)
            .is_some_and(|value| {
                MoltObject::from_bits(value.bits())
                    .as_ptr()
                    .is_some_and(|ptr| unsafe {
                        object_type_id(ptr) == TYPE_ID_MODULE
                            && crate::c_api::molt_module_capi_get_def(value.bits()) != 0
                    })
            });
        if !valid_module {
            with_preserved_error(|| unsafe { molt_cpython_abi::api::refcount::Py_DECREF(result) });
            return initialization_contract_violation(
                py,
                &format!("{name}: PyInit did not return an extension module"),
            );
        }
    }
    let def = if definition {
        result.cast::<PyModuleDef>()
    } else {
        ptr::null_mut()
    };
    let spec = extension_spec(py, name_bits, origin_bits, supplied_spec);
    let Some(spec) = spec else {
        if !definition {
            with_preserved_error(|| unsafe { molt_cpython_abi::api::refcount::Py_DECREF(result) });
        }
        return initialization_failure(
            py,
            &format!("{name}: PyModuleDef ModuleSpec bridge failed"),
        );
    };
    let module = if definition {
        if unsafe { (*def).m_name.is_null() || CStr::from_ptr((*def).m_name).to_bytes().is_empty() }
        {
            with_preserved_error(|| unsafe { molt_cpython_abi::api::refcount::Py_DECREF(spec) });
            return initialization_failure(
                py,
                "extension PyInit returned an invalid module definition",
            );
        }
        let module =
            unsafe { molt_cpython_abi::api::modules::PyModule_FromDefAndSpec2(def, spec, 0) };
        if module.is_null() {
            with_preserved_error(|| unsafe { molt_cpython_abi::api::refcount::Py_DECREF(spec) });
            return initialization_failure(py, &format!("{name}: PyModuleDef creation failed"));
        }
        module
    } else {
        result
    };
    let outcome = unsafe {
        module_result_to_bits(
            py,
            module,
            def,
            ModuleResultContext {
                name_bits,
                name: &name,
                spec,
                origin_bits,
                phase: if definition {
                    ExtensionPhase::MultiPhase {
                        execute: !create_only,
                    }
                } else {
                    ExtensionPhase::SinglePhase { reload }
                },
                import_identity,
            },
        )
    };
    with_preserved_error(|| unsafe { molt_cpython_abi::api::refcount::Py_DECREF(spec) });
    match outcome {
        Ok(bits) => bits,
        Err(message) => initialization_failure(
            py,
            if message.is_empty() {
                "extension module initialization failed"
            } else {
                &message
            },
        ),
    }
}

// Tests can inject malformed results without invoking undefined C behavior.
// Production callers can only enter through the atomic callback boundary.
#[cfg(test)]
pub(super) fn molt_cpython_abi_pyinit_module_to_bits(
    result_pyobj: u64,
    module_name_bits: u64,
) -> u64 {
    with_gil(|py| {
        initialize_result(
            &py,
            result_pyobj as *mut PyObject,
            ExtensionInitialization {
                name_bits: module_name_bits,
                origin_bits: MoltObject::none().bits(),
                supplied_spec: MoltObject::none().bits(),
                create_only: false,
                import_identity: None,
                reload: false,
            },
        )
    })
}

pub(super) unsafe extern "C" fn hook_initialize_extension(
    init: unsafe extern "C" fn() -> *mut PyObject,
    name_bits: u64,
    origin_bits: u64,
    spec_bits: u64,
    create_only: bool,
) -> OwnedHandleResult {
    let bits = with_gil(|py| unsafe {
        run_extension_init(&py, init, name_bits, origin_bits, spec_bits, create_only)
    });
    owned_result_from_pending(bits)
}

/// Execute the already-created physical C module, never a namespace copy.
/// Importlib owns sys.modules publication/restoration; this operation keeps
/// the runtime cache bound to that same identity while slots execute.
#[cfg(any(test, all(feature = "cext_loader", not(target_arch = "wasm32"))))]
pub(crate) fn execute_prepared_extension(
    module_bits: u64,
    name_bits: u64,
    publish_cache: bool,
) -> u64 {
    with_gil(|py| unsafe {
        if crate::c_api::module_exec_started(module_bits) {
            return MoltObject::none().bits();
        }
        let def = crate::c_api::molt_module_capi_get_def(module_bits) as *mut PyModuleDef;
        if crate::exception_pending(&py) {
            return MoltObject::none().bits();
        }
        if def.is_null() {
            return crate::raise_exception::<u64>(
                &py,
                "ImportError",
                "extension exec requires a module created from a C module definition",
            );
        }
        let Some(name) = crate::string_obj_to_owned(MoltObject::from_bits(name_bits)) else {
            return crate::raise_exception::<u64>(
                &py,
                "TypeError",
                "extension module name must be a str",
            );
        };
        let object =
            molt_cpython_abi::bridge::GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(module_bits);
        if object.is_null() {
            return initialization_failure(&py, "extension module ABI view unavailable");
        }
        let mut publication = ModulePublication {
            object,
            name_bits,
            def,
            register_state: false,
            published: false,
            committed: false,
        };
        if !publish_cache {
            // Direct loader execution does not participate in import caching.
            // Its caller retains both the module and any prior namespace entry.
            if molt_cpython_abi::api::modules::PyModule_ExecDef(object, def) != 0 {
                return initialization_failure(&py, &format!("{name}: Py_mod_exec failed"));
            }
            publication.committed = true;
            return MoltObject::none().bits();
        }
        if let Err(message) = publication.publish(
            &py,
            &name,
            module_bits,
            ExtensionPublicationAdmission::VacantOrSelf,
        ) {
            return initialization_failure(&py, &message);
        }
        if molt_cpython_abi::api::modules::PyModule_ExecDef(object, def) != 0 {
            return initialization_failure(&py, &format!("{name}: Py_mod_exec failed"));
        }
        publication.committed = true;
        MoltObject::none().bits()
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use molt_cpython_abi::abi_types::{
        METH_NOARGS, PyMethodDef, PyModuleDef_Base, PyModuleDef_Slot,
    };
    use molt_cpython_abi::api::{modules, object, refcount, strings};
    use std::cell::Cell;
    use std::ffi::c_void;

    thread_local! {
        static EXEC_CALLS: Cell<usize> = const { Cell::new(0) };
        static EXEC_SAW_CACHE: Cell<bool> = const { Cell::new(false) };
        static EXPECTED_SPEC: Cell<usize> = const { Cell::new(0) };
        static INIT_DEFS: Cell<[usize; 2]> = const { Cell::new([0; 2]) };
        static INIT_DEPTH: Cell<usize> = const { Cell::new(0) };
        static INNER_MODULE: Cell<u64> = const { Cell::new(0) };
        static EXTRA_MODULE: Cell<usize> = const { Cell::new(0) };
        static FAIL_INIT: Cell<bool> = const { Cell::new(false) };
        static REIMPORT_DEF: Cell<usize> = const { Cell::new(0) };
        static REIMPORT_CALLS: Cell<usize> = const { Cell::new(0) };
        static REIMPORT_FAIL: Cell<bool> = const { Cell::new(false) };
        static REIMPORT_MULTIPHASE: Cell<bool> = const { Cell::new(false) };
    }

    unsafe extern "C" fn single_phase_init() -> *mut PyObject {
        let depth = INIT_DEPTH.with(Cell::get);
        if depth == 0 {
            INIT_DEPTH.with(|depth| depth.set(1));
            let nested_name = unsafe { hook_alloc_str(b"inner.physical".as_ptr(), 14) };
            let nested = molt_cpython_abi_run_static_extension_init(
                single_phase_init as *const () as u64,
                nested_name,
            );
            INNER_MODULE.with(|slot| slot.set(nested));
            unsafe { hook_dec_ref(nested_name) };
            INIT_DEPTH.with(|depth| depth.set(0));
        }
        let def = INIT_DEFS.with(Cell::get)[depth] as *mut PyModuleDef;
        let module = unsafe { modules::PyModule_Create2(def, 0) };
        if depth == 0 {
            // The first matching allocation consumes the package context.
            let second = unsafe { modules::PyModule_Create2(def, 0) };
            EXTRA_MODULE.with(|slot| slot.set(second.addr()));
            if FAIL_INIT.with(Cell::get) {
                unsafe {
                    molt_cpython_abi::api::errors::PyErr_SetString(
                        (&raw mut molt_cpython_abi::abi_types::PyExc_TypeError).cast(),
                        c"initializer failed after nested import".as_ptr(),
                    );
                }
            }
        }
        module
    }

    fn single_definition(methods: *mut PyMethodDef) -> PyModuleDef {
        PyModuleDef {
            m_base: PyModuleDef_Base {
                ob_base: PyObject {
                    ob_refcnt: 1,
                    ob_type: ptr::null_mut(),
                },
                m_init: None,
                m_index: 0,
                m_copy: ptr::null_mut(),
            },
            m_name: c"physical".as_ptr(),
            m_doc: ptr::null(),
            m_size: -1,
            m_methods: methods,
            m_slots: ptr::null_mut(),
            m_traverse: ptr::null_mut(),
            m_clear: ptr::null_mut(),
            m_free: ptr::null_mut(),
        }
    }

    unsafe fn assert_native_module_name(module: *mut PyObject, expected: &str) {
        let name = unsafe { object::PyObject_GetAttrString(module, c"__name__".as_ptr()) };
        assert_eq!(
            unsafe { CStr::from_ptr(strings::PyUnicode_AsUTF8(name)) }
                .to_str()
                .unwrap(),
            expected
        );
        unsafe { refcount::Py_DECREF(name) };
        let method = unsafe { object::PyObject_GetAttrString(module, c"identity".as_ptr()) };
        assert!(!method.is_null());
        let physical = method.cast::<molt_cpython_abi::abi_types::PyCFunctionObject>();
        assert_eq!(unsafe { (*physical).m_self }, module);
        assert_eq!(
            unsafe { CStr::from_ptr(strings::PyUnicode_AsUTF8((*physical).m_module)) }
                .to_str()
                .unwrap(),
            expected
        );
        unsafe { refcount::Py_DECREF(method) };
    }

    unsafe extern "C" fn reimport_init() -> *mut PyObject {
        REIMPORT_CALLS.with(|calls| calls.set(calls.get() + 1));
        if REIMPORT_FAIL.with(Cell::get) {
            unsafe {
                molt_cpython_abi::api::errors::PyErr_SetString(
                    (&raw mut molt_cpython_abi::abi_types::PyExc_ValueError).cast(),
                    c"reimport initializer failure".as_ptr(),
                );
            }
            return ptr::null_mut();
        }
        let def = REIMPORT_DEF.with(Cell::get) as *mut PyModuleDef;
        if REIMPORT_MULTIPHASE.with(Cell::get) {
            return unsafe { modules::PyModuleDef_Init(def) };
        }
        let module = unsafe { modules::PyModule_Create2(def, 0) };
        if !module.is_null() {
            unsafe { modules::PyModule_AddIntConstant(module, c"value".as_ptr(), 7) };
        }
        module
    }

    unsafe extern "C" fn reimport_exec(module: *mut PyObject) -> c_int {
        EXEC_CALLS.with(|calls| calls.set(calls.get() + 1));
        unsafe { modules::PyModule_AddIntConstant(module, c"value".as_ptr(), 7) }
    }

    struct ExtensionPublicationProbe {
        name: u64,
        def: usize,
        identity: crate::c_api::ExtensionImportIdentity,
        reenter: bool,
        calls: Cell<usize>,
        registered: Cell<u64>,
        snapshot: Cell<bool>,
        nested: Cell<u64>,
    }

    unsafe extern "C" fn observe_extension_publication(capsule: *mut PyObject) {
        let probe =
            unsafe { molt_cpython_abi::api::capsule::PyCapsule_GetPointer(capsule, ptr::null()) }
                .cast::<ExtensionPublicationProbe>();
        let probe = unsafe { &*probe };
        with_gil(|py| unsafe {
            probe.calls.set(probe.calls.get() + 1);
            probe
                .registered
                .set(crate::c_api::molt_module_state_find(probe.def));
            let imported = crate::c_api::module_extension_import(&py, &probe.identity);
            probe
                .snapshot
                .set(imported.is_some_and(|(_, snapshot)| snapshot.is_some()));
            if let Some((_, Some(snapshot))) = imported {
                dec_ref_bits(&py, snapshot);
            }
            if probe.reenter {
                let nested = run_extension_init(
                    &py,
                    reimport_init,
                    probe.name,
                    MoltObject::none().bits(),
                    MoltObject::none().bits(),
                    true,
                );
                probe.nested.set(nested);
            }
        });
    }

    unsafe fn install_publication_probe(
        py: &crate::PyToken<'_>,
        modules: *mut u8,
        probe: &ExtensionPublicationProbe,
    ) {
        let module = unsafe { modules::PyModule_New(c"publication_observer".as_ptr()) };
        assert!(!module.is_null());
        let capsule = unsafe {
            molt_cpython_abi::api::capsule::PyCapsule_New(
                (probe as *const ExtensionPublicationProbe)
                    .cast_mut()
                    .cast(),
                ptr::null(),
                Some(observe_extension_publication),
            )
        };
        assert!(!capsule.is_null());
        assert_eq!(
            unsafe { modules::PyModule_AddObject(module, c"observer".as_ptr(), capsule) },
            0
        );
        let bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(module)
            .unwrap()
            .bits();
        unsafe { crate::dict_set_in_place(py, modules, probe.name, bits) };
        unsafe { refcount::Py_DECREF(module) };
    }

    #[test]
    fn extension_publication_callbacks_follow_target_and_initial_or_reload_phase() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(register_cpython_hooks());
        with_gil(|py| unsafe {
            let state = crate::runtime_state(&py);
            let previous = crate::object::ops_sys::runtime_target_python_info(state);
            let sys_name = hook_alloc_str(b"sys".as_ptr(), 3);
            let mut sys = crate::builtins::modules::molt_module_cache_get(sys_name);
            if MoltObject::from_bits(sys).is_none() {
                sys = crate::builtins::modules::molt_module_new(sys_name);
            }
            crate::builtins::module_table::publish_interpreter_sys_for_test(&py, sys);
            let modules = crate::builtins::modules::sys_modules_dict_bits(&py, sys).unwrap();
            let modules_ptr = MoltObject::from_bits(modules).as_ptr().unwrap();
            for minor in [12, 13, 14] {
                let mut target = previous.clone();
                target.minor = minor;
                *state.sys_version_info.lock().unwrap() = Some(target);
                for size in [-1, 0] {
                    let mut def = single_definition(ptr::null_mut());
                    def.m_size = size;
                    REIMPORT_DEF.with(|value| value.set((&raw mut def).addr()));
                    REIMPORT_MULTIPHASE.with(|value| value.set(false));
                    REIMPORT_FAIL.with(|value| value.set(false));
                    REIMPORT_CALLS.with(|value| value.set(0));
                    let name = format!(
                        "callback_{minor}_{}.physical",
                        if size == -1 { "legacy" } else { "repeat" }
                    );
                    let name_bits = hook_alloc_str(name.as_ptr(), name.len());
                    let probe = ExtensionPublicationProbe {
                        name: name_bits,
                        def: (&raw mut def).addr(),
                        identity: crate::c_api::ExtensionImportIdentity {
                            init_address: reimport_init as *const () as usize,
                            name,
                            origin: None,
                        },
                        reenter: size == -1,
                        calls: Cell::new(0),
                        registered: Cell::new(0),
                        snapshot: Cell::new(false),
                        nested: Cell::new(0),
                    };
                    install_publication_probe(&py, modules_ptr, &probe);
                    let first = run_extension_init(
                        &py,
                        reimport_init,
                        name_bits,
                        MoltObject::none().bits(),
                        MoltObject::none().bits(),
                        true,
                    );
                    assert!(!crate::exception_pending(&py));
                    assert_eq!(
                        probe.calls.get(),
                        1,
                        "public replacement must release the displaced owner"
                    );
                    assert_eq!(probe.registered.get(), if minor >= 13 { first } else { 0 });
                    assert_eq!(probe.snapshot.get(), minor >= 13 && size == -1);
                    if size == -1 {
                        assert_eq!(
                            REIMPORT_CALLS.with(Cell::get),
                            if minor >= 13 { 1 } else { 2 }
                        );
                        assert_eq!(probe.nested.get() == first, minor >= 13);
                        dec_ref_bits(&py, probe.nested.get());
                    } else {
                        // PyState removal must not turn an extension-cache
                        // reload into a first initialization on 3.13+.
                        assert_eq!(modules::PyState_RemoveModule(&raw mut def), 0);
                        assert!(
                            crate::c_api::module_extension_import(&py, &probe.identity).is_some()
                        );
                        probe.calls.set(0);
                        probe.registered.set(u64::MAX);
                        install_publication_probe(&py, modules_ptr, &probe);
                        let second = run_extension_init(
                            &py,
                            reimport_init,
                            name_bits,
                            MoltObject::none().bits(),
                            MoltObject::none().bits(),
                            true,
                        );
                        assert!(!crate::exception_pending(&py));
                        assert_eq!(probe.calls.get(), 1);
                        assert_eq!(
                            probe.registered.get(),
                            0,
                            "repeat publication precedes registration on every version"
                        );
                        assert_eq!(
                            crate::c_api::molt_module_state_find((&raw mut def).addr()),
                            second
                        );
                        dec_ref_bits(&py, second);
                    }
                    crate::builtins::modules::molt_module_cache_del(name_bits);
                    dec_ref_bits(&py, first);
                    assert!(crate::c_api::c_api_module_clear_state(&py, state));
                    dec_ref_bits(&py, name_bits);
                }
            }
            *state.sys_version_info.lock().unwrap() = Some(previous);
            for bits in [modules, sys, sys_name] {
                dec_ref_bits(&py, bits);
            }
        });
    }

    #[test]
    fn multiphase_exec_replaces_a_stale_private_module() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(register_cpython_hooks());
        with_gil(|py| unsafe {
            let sys_name = hook_alloc_str(b"sys".as_ptr(), 3);
            let mut sys = crate::builtins::modules::molt_module_cache_get(sys_name);
            if MoltObject::from_bits(sys).is_none() {
                sys = crate::builtins::modules::molt_module_new(sys_name);
            }
            crate::builtins::module_table::publish_interpreter_sys_for_test(&py, sys);
            let modules = crate::builtins::modules::sys_modules_dict_bits(&py, sys).unwrap();
            let modules_ptr = MoltObject::from_bits(modules).as_ptr().unwrap();
            let text = "multiphase_stale_private.physical";
            let name = hook_alloc_str(text.as_ptr(), text.len());
            let old = crate::builtins::modules::molt_module_new(name);
            crate::builtins::modules::molt_module_cache_set(name, old);
            assert!(crate::object::ops::dict_del_in_place(
                &py,
                modules_ptr,
                name
            ));
            let mut slots = [
                PyModuleDef_Slot {
                    slot: 2,
                    value: reimport_exec as *mut c_void,
                },
                PyModuleDef_Slot {
                    slot: 0,
                    value: ptr::null_mut(),
                },
            ];
            let mut def = single_definition(ptr::null_mut());
            def.m_size = 0;
            def.m_slots = slots.as_mut_ptr();
            REIMPORT_DEF.with(|value| value.set((&raw mut def).addr()));
            REIMPORT_MULTIPHASE.with(|value| value.set(true));
            REIMPORT_FAIL.with(|value| value.set(false));
            EXEC_CALLS.with(|value| value.set(0));
            let created = run_extension_init(
                &py,
                reimport_init,
                name,
                MoltObject::none().bits(),
                MoltObject::none().bits(),
                true,
            );
            assert!(!crate::exception_pending(&py));
            assert_eq!(dict_get_in_place(&py, modules_ptr, name), None);
            execute_prepared_extension(created, name, true);
            assert!(!crate::exception_pending(&py));
            assert_eq!(EXEC_CALLS.with(Cell::get), 1);
            assert_eq!(dict_get_in_place(&py, modules_ptr, name), Some(created));
            let cache = crate::builtins::exceptions::internals::module_cache(&py);
            assert_eq!(cache.lock().unwrap().get(text).copied(), Some(created));
            crate::builtins::modules::module_cache_publish(
                name,
                old,
                crate::builtins::modules::ModuleCachePublication::Extension,
            );
            let already_public = run_extension_init(
                &py,
                reimport_init,
                name,
                MoltObject::none().bits(),
                MoltObject::none().bits(),
                true,
            );
            assert!(!crate::exception_pending(&py));
            crate::dict_set_in_place(&py, modules_ptr, name, already_public);
            execute_prepared_extension(already_public, name, true);
            assert!(!crate::exception_pending(&py));
            assert_eq!(EXEC_CALLS.with(Cell::get), 2);
            assert_eq!(
                dict_get_in_place(&py, modules_ptr, name),
                Some(already_public)
            );
            assert_eq!(
                cache.lock().unwrap().get(text).copied(),
                Some(already_public),
                "an already-public result still replaces stale private ownership"
            );
            let def_address = (&raw mut def).addr();
            for bits in [created, already_public] {
                assert_eq!(crate::c_api::molt_module_capi_get_def(bits), def_address);
            }
            assert_eq!(
                crate::c_api::molt_module_state_find(def_address),
                0,
                "multiphase modules have metadata but no single-phase PyState root"
            );
            crate::builtins::modules::molt_module_cache_del(name);
            for bits in [already_public, created, old, name, modules, sys, sys_name] {
                dec_ref_bits(&py, bits);
            }
            assert!(
                !crate::c_api::c_api_module_clear_state(&py, crate::runtime_state(&py)),
                "final module release must already retire multiphase metadata"
            );
        });
    }

    #[test]
    fn extension_import_identity_transfers_between_definitions_atomically() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(register_cpython_hooks());
        with_gil(|py| unsafe {
            let text = "unique_extension_identity";
            let name = hook_alloc_str(text.as_ptr(), text.len());
            let first = crate::builtins::modules::molt_module_new(name);
            let second = crate::builtins::modules::molt_module_new(name);
            let first_dict = crate::molt_dict_copy(crate::object::layout::module_dict_bits(
                MoltObject::from_bits(first).as_ptr().unwrap(),
            ));
            let second_dict = crate::molt_dict_copy(crate::object::layout::module_dict_bits(
                MoltObject::from_bits(second).as_ptr().unwrap(),
            ));
            let mut defs = [
                single_definition(ptr::null_mut()),
                single_definition(ptr::null_mut()),
            ];
            let a = (&raw mut defs[0]).addr();
            let b = (&raw mut defs[1]).addr();
            let identity = crate::c_api::ExtensionImportIdentity {
                init_address: reimport_init as *const () as usize,
                name: text.into(),
                origin: None,
            };
            assert_eq!(
                crate::c_api::module_state_add_with_import(
                    first,
                    a,
                    Some((&identity, Some(first_dict)))
                ),
                0
            );
            assert_eq!(
                crate::c_api::module_state_add_with_import(
                    second,
                    b,
                    Some((&identity, Some(second_dict)))
                ),
                0
            );
            let (def, snapshot) = crate::c_api::module_extension_import(&py, &identity).unwrap();
            assert_eq!(def, b);
            assert_eq!(snapshot, Some(second_dict));
            dec_ref_bits(&py, snapshot.unwrap());
            assert_eq!(
                crate::c_api::molt_module_state_find(a),
                first,
                "cache-key transfer does not remove independent PyState ownership"
            );
            let ptr = MoltObject::from_bits(first_dict).as_ptr().unwrap();
            assert_eq!(
                (*crate::object::header_from_obj_ptr(ptr)).ref_count_snapshot(),
                1,
                "the displaced identity releases its orphaned snapshot"
            );
            assert_eq!(crate::c_api::molt_module_state_remove(a), 0);
            assert_eq!(crate::c_api::molt_module_state_remove(b), 0);
            let (def, snapshot) = crate::c_api::module_extension_import(&py, &identity).unwrap();
            assert_eq!(def, b);
            dec_ref_bits(&py, snapshot.unwrap());
            assert!(crate::c_api::c_api_module_clear_state(
                &py,
                crate::runtime_state(&py)
            ));
            for bits in [first_dict, second_dict, first, second, name] {
                dec_ref_bits(&py, bits);
            }
        });
    }
    #[test]
    fn extension_create_module_publication_follows_the_initialization_phase() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(register_cpython_hooks());
        with_gil(|py| unsafe {
            let sys_name = hook_alloc_str(b"sys".as_ptr(), 3);
            let mut sys = crate::builtins::modules::molt_module_cache_get(sys_name);
            if MoltObject::from_bits(sys).is_none() {
                sys = crate::builtins::modules::molt_module_new(sys_name);
            }
            assert!(!crate::exception_pending(&py));
            crate::builtins::module_table::publish_interpreter_sys_for_test(&py, sys);
            let modules = crate::builtins::modules::sys_modules_dict_bits(&py, sys).unwrap();
            let modules_ptr = MoltObject::from_bits(modules).as_ptr().unwrap();
            dec_ref_bits(&py, sys_name);
            dec_ref_bits(&py, sys);
            for (label, size, multiphase) in [
                ("legacy", -1, false),
                ("single", 0, false),
                ("multi", 0, true),
            ] {
                let mut slots = [
                    PyModuleDef_Slot {
                        slot: 2,
                        value: reimport_exec as *mut c_void,
                    },
                    PyModuleDef_Slot {
                        slot: 0,
                        value: ptr::null_mut(),
                    },
                ];
                let mut def = single_definition(ptr::null_mut());
                def.m_size = size;
                if multiphase {
                    def.m_slots = slots.as_mut_ptr();
                }
                REIMPORT_DEF.with(|value| value.set((&raw mut def).addr()));
                REIMPORT_MULTIPHASE.with(|value| value.set(multiphase));
                REIMPORT_CALLS.with(|value| value.set(0));
                REIMPORT_FAIL.with(|value| value.set(false));
                EXEC_CALLS.with(|value| value.set(0));
                let name = format!("created_{label}.physical");
                let name_bits = hook_alloc_str(name.as_ptr(), name.len());
                let first = run_extension_init(
                    &py,
                    reimport_init,
                    name_bits,
                    MoltObject::none().bits(),
                    MoltObject::none().bits(),
                    true,
                );
                assert!(!crate::exception_pending(&py));
                assert!(!MoltObject::from_bits(first).is_none());
                assert_eq!(
                    dict_get_in_place(&py, modules_ptr, name_bits),
                    if multiphase { None } else { Some(first) }
                );
                // CPython _imp.create_dynamic publishes an executed single-
                // phase result even on the very first create_module call.
                let cached = crate::builtins::modules::molt_module_cache_get(name_bits);
                assert_eq!(
                    cached,
                    if multiphase {
                        MoltObject::none().bits()
                    } else {
                        first
                    }
                );
                dec_ref_bits(&py, cached);
                let second = run_extension_init(
                    &py,
                    reimport_init,
                    name_bits,
                    MoltObject::none().bits(),
                    MoltObject::none().bits(),
                    true,
                );
                assert!(!crate::exception_pending(&py));
                assert_eq!(first == second, size == -1);
                assert_eq!(
                    REIMPORT_CALLS.with(Cell::get),
                    if size == -1 { 1 } else { 2 }
                );
                assert_eq!(EXEC_CALLS.with(Cell::get), 0);
                assert_eq!(
                    dict_get_in_place(&py, modules_ptr, name_bits),
                    if multiphase { None } else { Some(second) }
                );
                if !multiphase {
                    // The trusted initializer owns private publication too;
                    // first-init-wins must not republish its predecessor.
                    let private = crate::builtins::exceptions::internals::module_cache(&py);
                    assert_eq!(private.lock().unwrap().get(&name).copied(), Some(second));
                    let object =
                        molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(second);
                    assert_eq!(modules::PyState_FindModule(&raw mut def), object);
                }
                let _ = execute_prepared_extension(second, name_bits, false);
                assert!(!crate::exception_pending(&py));
                assert_eq!(EXEC_CALLS.with(Cell::get), usize::from(multiphase));
                assert_eq!(
                    dict_get_in_place(&py, modules_ptr, name_bits),
                    if multiphase { None } else { Some(second) }
                );
                if size == 0 && !multiphase {
                    // A failed rerun has not published anything and cannot
                    // remove the previous successful create_module result.
                    REIMPORT_FAIL.with(|value| value.set(true));
                    let failed = run_extension_init(
                        &py,
                        reimport_init,
                        name_bits,
                        MoltObject::none().bits(),
                        MoltObject::none().bits(),
                        true,
                    );
                    assert!(MoltObject::from_bits(failed).is_none());
                    assert_eq!(
                        super::super::tests::pending_exception_type_for_assertion(),
                        "ValueError"
                    );
                    crate::clear_exception(&py);
                    assert_eq!(dict_get_in_place(&py, modules_ptr, name_bits), Some(second));

                    // Simulate public replacement after only some transaction
                    // owners were installed. Rollback must retire our private
                    // owner without deleting this different public identity.
                    let replacement_ptr = alloc_module_obj(&py, name_bits);
                    assert!(!replacement_ptr.is_null());
                    let replacement = MoltObject::from_ptr(replacement_ptr).bits();
                    crate::dict_set_in_place(&py, modules_ptr, name_bits, replacement);
                    let object = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .borrowed_handle_to_new_pyobj(second);
                    assert!(!object.is_null());
                    drop(ModulePublication {
                        object,
                        name_bits,
                        def: &raw mut def,
                        register_state: false,
                        published: true,
                        committed: false,
                    });
                    assert_eq!(
                        dict_get_in_place(&py, modules_ptr, name_bits),
                        Some(replacement)
                    );
                    let private = crate::builtins::exceptions::internals::module_cache(&py);
                    assert!(!private.lock().unwrap().contains_key(&name));
                    dec_ref_bits(&py, replacement);
                }
                let def_address = (&raw mut def).addr();
                for bits in [first, second] {
                    assert_eq!(crate::c_api::molt_module_capi_get_def(bits), def_address);
                }
                if multiphase {
                    assert_eq!(crate::c_api::molt_module_state_find(def_address), 0);
                }
                let _ = crate::builtins::modules::molt_module_cache_del(name_bits);
                dec_ref_bits(&py, first);
                dec_ref_bits(&py, second);
                assert_eq!(
                    crate::c_api::c_api_module_clear_state(&py, crate::runtime_state(&py)),
                    !multiphase,
                    "only single-phase import roots outlive these final module releases"
                );
                assert_eq!(crate::c_api::molt_module_state_find(def_address), 0);
                dec_ref_bits(&py, name_bits);
                let _ = molt_cpython_abi::api::memory::PyGC_Collect();
                assert!(!crate::exception_pending(&py));
            }
            dec_ref_bits(&py, modules);
        });
    }

    #[test]
    fn extension_reimport_uses_phase_contract_and_survives_pystate_removal() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(register_cpython_hooks());
        with_gil(|py| unsafe {
            // Exercise the real public import cache, as an initialized guest
            // does. The transaction guard resets this bootstrap after the test.
            let sys_name = hook_alloc_str(b"sys".as_ptr(), 3);
            let mut sys = crate::builtins::modules::molt_module_cache_get(sys_name);
            if MoltObject::from_bits(sys).is_none() {
                sys = crate::builtins::modules::molt_module_new(sys_name);
            }
            crate::builtins::module_table::publish_interpreter_sys_for_test(&py, sys);
            assert!(!crate::exception_pending(&py));
            dec_ref_bits(&py, sys_name);
            dec_ref_bits(&py, sys);
            for (label, size, multiphase) in [
                ("legacy", -1, false),
                ("single", 0, false),
                ("multi", 0, true),
            ] {
                let mut methods = [
                    PyMethodDef {
                        ml_name: c"identity".as_ptr(),
                        ml_meth: Some(identity),
                        ml_flags: METH_NOARGS,
                        ml_doc: ptr::null(),
                    },
                    PyMethodDef {
                        ml_name: ptr::null(),
                        ml_meth: None,
                        ml_flags: 0,
                        ml_doc: ptr::null(),
                    },
                ];
                let mut slots = [
                    PyModuleDef_Slot {
                        slot: 2,
                        value: reimport_exec as *mut c_void,
                    },
                    PyModuleDef_Slot {
                        slot: 0,
                        value: ptr::null_mut(),
                    },
                ];
                let mut def = single_definition(methods.as_mut_ptr());
                def.m_size = size;
                if multiphase {
                    def.m_slots = slots.as_mut_ptr();
                }
                REIMPORT_DEF.with(|value| value.set((&raw mut def).addr()));
                REIMPORT_MULTIPHASE.with(|value| value.set(multiphase));
                REIMPORT_CALLS.with(|value| value.set(0));
                REIMPORT_FAIL.with(|value| value.set(true));
                EXEC_CALLS.with(|value| value.set(0));
                let name = format!("phase_{label}.physical");
                let name_bits = hook_alloc_str(name.as_ptr(), name.len());
                let identity_key = crate::c_api::ExtensionImportIdentity {
                    init_address: reimport_init as *const () as usize,
                    name,
                    origin: None,
                };
                let failed = molt_cpython_abi_run_static_extension_init(
                    reimport_init as *const () as u64,
                    name_bits,
                );
                assert!(MoltObject::from_bits(failed).is_none());
                assert_eq!(
                    super::super::tests::pending_exception_type_for_assertion(),
                    "ValueError"
                );
                crate::clear_exception(&py);
                assert!(crate::c_api::module_extension_import(&py, &identity_key).is_none());
                assert!(
                    MoltObject::from_bits(crate::builtins::modules::molt_module_cache_get(
                        name_bits
                    ))
                    .is_none()
                );
                assert_eq!(
                    crate::c_api::molt_module_state_find((&raw mut def).addr()),
                    0
                );

                REIMPORT_FAIL.with(|value| value.set(false));
                let first = molt_cpython_abi_run_static_extension_init(
                    reimport_init as *const () as u64,
                    name_bits,
                );
                assert!(!crate::exception_pending(&py));
                assert_eq!(REIMPORT_CALLS.with(Cell::get), 2);
                assert!(module_set_bits(
                    first,
                    b"value",
                    MoltObject::from_int(99).bits()
                ));
                if size == -1 {
                    // module_from_spec(find_spec(name)) calls create_module
                    // again while the old public module is still present.
                    let warm = run_extension_init(
                        &py,
                        reimport_init,
                        name_bits,
                        MoltObject::none().bits(),
                        MoltObject::none().bits(),
                        true,
                    );
                    assert!(!crate::exception_pending(&py));
                    assert_eq!(warm, first);
                    assert_eq!(REIMPORT_CALLS.with(Cell::get), 2);
                    assert_eq!(
                        crate::c_api::molt_module_capi_get_def(warm),
                        (&raw mut def).addr()
                    );
                    let key = hook_alloc_str(b"value".as_ptr(), 5);
                    assert_eq!(
                        crate::builtins::modules::molt_module_get_attr(warm, key),
                        MoltObject::from_int(7).bits()
                    );
                    dec_ref_bits(&py, key);
                    dec_ref_bits(&py, warm);
                    assert!(module_set_bits(
                        first,
                        b"value",
                        MoltObject::from_int(99).bits()
                    ));
                }
                let _ = crate::builtins::modules::molt_module_cache_del(name_bits);
                if !multiphase {
                    assert_eq!(modules::PyState_RemoveModule(&raw mut def), 0);
                }
                if size == -1 {
                    let modules =
                        crate::builtins::modules::sys_modules_dict_bits(&py, sys).unwrap();
                    let modules_ptr = MoltObject::from_bits(modules).as_ptr().unwrap();
                    let replacement_ptr = alloc_module_obj(&py, name_bits);
                    assert!(!replacement_ptr.is_null());
                    let replacement = MoltObject::from_ptr(replacement_ptr).bits();
                    crate::dict_set_in_place(&py, modules_ptr, name_bits, replacement);
                    let reused = run_extension_init(
                        &py,
                        reimport_init,
                        name_bits,
                        MoltObject::none().bits(),
                        MoltObject::none().bits(),
                        true,
                    );
                    assert!(!crate::exception_pending(&py));
                    assert_eq!(
                        reused, replacement,
                        "public ModuleType replacement owns replay identity"
                    );
                    assert_eq!(crate::c_api::molt_module_capi_get_def(reused), 0);
                    dec_ref_bits(&py, reused);
                    assert!(crate::object::ops::dict_del_in_place(
                        &py,
                        modules_ptr,
                        name_bits
                    ));
                    assert_eq!(modules::PyState_RemoveModule(&raw mut def), 0);
                    dec_ref_bits(&py, replacement);

                    crate::dict_set_in_place(
                        &py,
                        modules_ptr,
                        name_bits,
                        MoltObject::none().bits(),
                    );
                    let replaced = run_extension_init(
                        &py,
                        reimport_init,
                        name_bits,
                        MoltObject::none().bits(),
                        MoltObject::none().bits(),
                        true,
                    );
                    assert!(!crate::exception_pending(&py));
                    assert!(!MoltObject::from_bits(replaced).is_none());
                    let cached = crate::builtins::modules::molt_module_cache_get(name_bits);
                    assert_eq!(
                        cached, replaced,
                        "create-only replay publishes before returning"
                    );
                    dec_ref_bits(&py, cached);
                    assert!(crate::object::ops::dict_del_in_place(
                        &py,
                        modules_ptr,
                        name_bits
                    ));
                    assert_eq!(modules::PyState_RemoveModule(&raw mut def), 0);
                    dec_ref_bits(&py, replaced);
                    dec_ref_bits(&py, modules);

                    let original =
                        molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(first);
                    // Drop the caller too: only the runtime-owned snapshot
                    // may keep the original module/method cycle reachable.
                    dec_ref_bits(&py, first);
                    let _ = molt_cpython_abi::api::memory::PyGC_Collect();
                    assert!(
                        molt_cpython_abi::bridge::GLOBAL_BRIDGE
                            .molt_handle_for_pyobj(original)
                            .is_some()
                    );
                }
                // Neither public deletion nor PyState removal discards the
                // legacy snapshot. Reinitializable definitions still rerun and
                // preserve exact rerun failures, leaving no publication.
                if size != -1 {
                    REIMPORT_FAIL.with(|value| value.set(true));
                    let failed = molt_cpython_abi_run_static_extension_init(
                        reimport_init as *const () as u64,
                        name_bits,
                    );
                    assert!(MoltObject::from_bits(failed).is_none());
                    assert_eq!(
                        super::super::tests::pending_exception_type_for_assertion(),
                        "ValueError"
                    );
                    crate::clear_exception(&py);
                    assert!(
                        MoltObject::from_bits(crate::builtins::modules::molt_module_cache_get(
                            name_bits
                        ))
                        .is_none()
                    );
                    assert_eq!(
                        crate::c_api::molt_module_state_find((&raw mut def).addr()),
                        0
                    );
                    REIMPORT_FAIL.with(|value| value.set(false));
                }
                let second = molt_cpython_abi_run_static_extension_init(
                    reimport_init as *const () as u64,
                    name_bits,
                );
                assert!(!crate::exception_pending(&py));
                assert_ne!(first, second);
                assert_eq!(
                    REIMPORT_CALLS.with(Cell::get),
                    if size == -1 { 2 } else { 4 }
                );
                if multiphase {
                    assert_eq!(EXEC_CALLS.with(Cell::get), 2);
                }
                let value_name = hook_alloc_str(b"value".as_ptr(), 5);
                let value = crate::builtins::modules::molt_module_get_attr(second, value_name);
                assert_eq!(
                    value,
                    MoltObject::from_int(7).bits(),
                    "post-init mutation leaked into reimport"
                );
                dec_ref_bits(&py, value_name);
                let current =
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(second);
                let original =
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(first);
                let method = object::PyObject_GetAttrString(current, c"identity".as_ptr());
                let receiver = object::PyObject_CallNoArgs(method);
                assert_eq!(receiver, if size == -1 { original } else { current });
                refcount::Py_DECREF(receiver);
                refcount::Py_DECREF(method);
                if size == -1 {
                    assert_eq!(crate::c_api::molt_module_capi_get_def(second), 0);
                    let _ = execute_prepared_extension(second, name_bits, false);
                    assert!(!crate::exception_pending(&py));
                }
                let imported = crate::c_api::module_extension_import(&py, &identity_key);
                assert_eq!(imported.is_some(), !multiphase);
                let snapshot =
                    imported.and_then(|(def, snapshot)| snapshot.map(|bits| (def, bits)));
                assert_eq!(snapshot.is_some(), size == -1);
                let _ = crate::builtins::modules::molt_module_cache_del(name_bits);
                if size != -1 {
                    dec_ref_bits(&py, first);
                }
                dec_ref_bits(&py, second);
                assert!(crate::c_api::c_api_module_clear_state(
                    &py,
                    crate::runtime_state(&py)
                ));
                assert!(crate::c_api::module_extension_import(&py, &identity_key).is_none());
                if let Some((_, snapshot)) = snapshot {
                    let ptr = MoltObject::from_bits(snapshot).as_ptr().unwrap();
                    assert_eq!(
                        (*crate::object::header_from_obj_ptr(ptr)).ref_count_snapshot(),
                        1,
                        "shutdown must release the runtime snapshot owner"
                    );
                    dec_ref_bits(&py, snapshot);
                }
                dec_ref_bits(&py, name_bits);
                let _ = molt_cpython_abi::api::memory::PyGC_Collect();
                assert!(!crate::exception_pending(&py));
            }
        });
    }

    #[test]
    fn atomic_single_phase_init_scopes_names_and_failed_results_across_nested_imports() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(register_cpython_hooks());
        with_gil(|py| unsafe {
            let mut methods = [
                PyMethodDef {
                    ml_name: c"identity".as_ptr(),
                    ml_meth: Some(identity),
                    ml_flags: METH_NOARGS,
                    ml_doc: ptr::null(),
                },
                PyMethodDef {
                    ml_name: ptr::null(),
                    ml_meth: None,
                    ml_flags: 0,
                    ml_doc: ptr::null(),
                },
            ];
            let mut defs = [
                single_definition(methods.as_mut_ptr()),
                single_definition(methods.as_mut_ptr()),
            ];
            for def in &mut defs {
                def.m_size = 0;
            }
            INIT_DEFS.with(|slot| slot.set([(&raw mut defs[0]).addr(), (&raw mut defs[1]).addr()]));
            let name = hook_alloc_str(b"outer.physical".as_ptr(), 14);
            let inner_name = hook_alloc_str(b"inner.physical".as_ptr(), 14);
            for fail in [false, true] {
                FAIL_INIT.with(|slot| slot.set(fail));
                let bits = molt_cpython_abi_run_static_extension_init(
                    single_phase_init as *const () as u64,
                    name,
                );
                let inner = INNER_MODULE.with(|slot| slot.replace(0));
                let extra = EXTRA_MODULE.with(|slot| slot.replace(0)) as *mut PyObject;
                if fail {
                    assert!(MoltObject::from_bits(bits).is_none());
                    assert_eq!(
                        super::super::tests::pending_exception_type_for_assertion(),
                        "SystemError"
                    );
                    crate::clear_exception(&py);
                    assert!(modules::PyState_FindModule(&raw mut defs[0]).is_null());
                } else {
                    let module =
                        molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits);
                    assert_native_module_name(module, "outer.physical");
                    assert_eq!(modules::PyState_FindModule(&raw mut defs[0]), module);
                    assert_eq!(modules::PyState_RemoveModule(&raw mut defs[0]), 0);
                    dec_ref_bits(&py, bits);
                }
                assert_native_module_name(
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(inner),
                    "inner.physical",
                );
                assert_native_module_name(extra, "physical");
                assert_eq!(modules::PyState_RemoveModule(&raw mut defs[1]), 0);
                refcount::Py_DECREF(extra);
                dec_ref_bits(&py, inner);
                let _ = crate::builtins::modules::molt_module_cache_del(name);
                let _ = crate::builtins::modules::molt_module_cache_del(inner_name);
                let unscoped = modules::PyModule_Create2(&raw mut defs[0], 0);
                assert_native_module_name(unscoped, "physical");
                refcount::Py_DECREF(unscoped);
                let _ = molt_cpython_abi::api::memory::PyGC_Collect();
                assert!(!crate::exception_pending(&py));
            }
            dec_ref_bits(&py, name);
            dec_ref_bits(&py, inner_name);
        });
    }

    unsafe extern "C" fn create(spec: *mut PyObject, _def: *mut PyModuleDef) -> *mut PyObject {
        assert_eq!(spec.addr(), EXPECTED_SPEC.with(Cell::get));
        let name = unsafe { object::PyObject_GetAttrString(spec, c"name".as_ptr()) };
        let module = unsafe { modules::PyModule_NewObject(name) };
        unsafe { refcount::Py_DECREF(name) };
        module
    }

    unsafe extern "C" fn identity(receiver: *mut PyObject, _args: *mut PyObject) -> *mut PyObject {
        unsafe { refcount::Py_INCREF(receiver) };
        receiver
    }

    unsafe extern "C" fn exec(module: *mut PyObject) -> c_int {
        EXEC_CALLS.with(|calls| calls.set(calls.get() + 1));
        let bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
            .molt_handle_for_pyobj(module)
            .unwrap()
            .bits();
        with_gil(|py| unsafe {
            let ptr = MoltObject::from_bits(bits).as_ptr().unwrap();
            let cached = crate::builtins::modules::molt_module_cache_get(
                crate::object::layout::module_name_bits(ptr),
            );
            EXEC_SAW_CACHE.with(|seen| seen.set(cached == bits));
            dec_ref_bits(&py, cached);
        });
        0
    }

    #[test]
    fn extension_create_exec_share_physical_identity_and_preserve_direct_exec_custody() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        assert!(register_cpython_hooks());
        for (custom_create, publish) in [(false, false), (false, true), (true, false), (true, true)]
        {
            EXEC_CALLS.with(|calls| calls.set(0));
            EXEC_SAW_CACHE.with(|seen| seen.set(false));
            with_gil(|py| unsafe {
                let name = format!("pkg.physical_{custom_create}_{publish}");
                let name_bits = hook_alloc_str(name.as_ptr(), name.len());
                let spec = module_spec(&py, name_bits, MoltObject::none().bits()).unwrap();
                if custom_create {
                    let locations = molt_cpython_abi::api::sequences::PyList_New(0);
                    assert!(!locations.is_null());
                    assert_eq!(
                        object::PyObject_SetAttrString(
                            spec,
                            c"submodule_search_locations".as_ptr(),
                            locations,
                        ),
                        0
                    );
                    refcount::Py_DECREF(locations);
                }
                let spec_bits = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                    .molt_handle_for_pyobj(spec)
                    .unwrap()
                    .bits();
                EXPECTED_SPEC.with(|expected| expected.set(spec.addr()));
                let mut methods = [
                    PyMethodDef {
                        ml_name: c"identity".as_ptr(),
                        ml_meth: Some(identity),
                        ml_flags: METH_NOARGS,
                        ml_doc: ptr::null(),
                    },
                    PyMethodDef {
                        ml_name: ptr::null(),
                        ml_meth: None,
                        ml_flags: 0,
                        ml_doc: ptr::null(),
                    },
                ];
                let mut slots = [
                    PyModuleDef_Slot {
                        slot: 2,
                        value: exec as *mut c_void,
                    },
                    PyModuleDef_Slot {
                        slot: if custom_create { 1 } else { 0 },
                        value: create as *mut c_void,
                    },
                    PyModuleDef_Slot {
                        slot: 0,
                        value: ptr::null_mut(),
                    },
                ];
                let mut def = PyModuleDef {
                    m_base: PyModuleDef_Base {
                        ob_base: PyObject {
                            ob_refcnt: 1,
                            ob_type: &raw mut PyModuleDef_Type,
                        },
                        m_init: None,
                        m_index: 0,
                        m_copy: ptr::null_mut(),
                    },
                    m_name: c"physical".as_ptr(),
                    m_doc: ptr::null(),
                    m_size: 1,
                    m_methods: methods.as_mut_ptr(),
                    m_slots: slots.as_mut_ptr(),
                    m_traverse: ptr::null_mut(),
                    m_clear: ptr::null_mut(),
                    m_free: ptr::null_mut(),
                };
                let bits = initialize_result(
                    &py,
                    (&raw mut def).cast(),
                    ExtensionInitialization {
                        name_bits,
                        origin_bits: MoltObject::none().bits(),
                        supplied_spec: spec_bits,
                        create_only: true,
                        import_identity: None,
                        reload: false,
                    },
                );
                assert!(!crate::exception_pending(&py));
                assert_eq!(
                    EXEC_CALLS.with(Cell::get),
                    0,
                    "create must not execute slots"
                );
                assert!(
                    MoltObject::from_bits(crate::builtins::modules::molt_module_cache_get(
                        name_bits
                    ))
                    .is_none()
                );
                let module = molt_cpython_abi::bridge::GLOBAL_BRIDGE.handle_to_borrowed_pyobj(bits);
                let actual_spec = object::PyObject_GetAttrString(module, c"__spec__".as_ptr());
                assert_eq!(actual_spec, spec);
                refcount::Py_DECREF(actual_spec);
                let package = object::PyObject_GetAttrString(module, c"__package__".as_ptr());
                assert!(!package.is_null());
                assert_eq!(
                    CStr::from_ptr(strings::PyUnicode_AsUTF8(package))
                        .to_str()
                        .unwrap(),
                    if custom_create { name.as_str() } else { "pkg" },
                    "the actual spec, including package status, owns __package__"
                );
                refcount::Py_DECREF(package);
                let method = object::PyObject_GetAttrString(module, c"identity".as_ptr());
                assert!(!method.is_null(), "both constructors must attach m_methods");
                let physical = method.cast::<molt_cpython_abi::abi_types::PyCFunctionObject>();
                assert_eq!((*physical).m_self, module);
                assert_eq!(
                    CStr::from_ptr(strings::PyUnicode_AsUTF8((*physical).m_module))
                        .to_str()
                        .unwrap(),
                    name
                );
                let returned = object::PyObject_CallNoArgs(method);
                assert_eq!(
                    returned, module,
                    "native callback self is the imported module"
                );
                refcount::Py_DECREF(returned);
                for _ in 0..2 {
                    let _ = execute_prepared_extension(bits, name_bits, publish);
                    assert!(!crate::exception_pending(&py));
                }
                assert_eq!(
                    EXEC_CALLS.with(Cell::get),
                    1,
                    "exec is idempotent per module"
                );
                assert_eq!(EXEC_SAW_CACHE.with(Cell::get), publish);
                let cached = crate::builtins::modules::molt_module_cache_get(name_bits);
                if publish {
                    assert_eq!(cached, bits);
                } else {
                    assert!(MoltObject::from_bits(cached).is_none());
                }
                dec_ref_bits(&py, cached);
                let _ = crate::builtins::modules::molt_module_cache_del(name_bits);
                refcount::Py_DECREF(method);
                refcount::Py_DECREF(spec);
                dec_ref_bits(&py, bits);
                dec_ref_bits(&py, name_bits);
                let _ = molt_cpython_abi::api::memory::PyGC_Collect();
                assert!(
                    molt_cpython_abi::bridge::GLOBAL_BRIDGE
                        .molt_handle_for_pyobj(module)
                        .is_none(),
                    "module/method cycles must release their physical views"
                );
            });
        }
    }
}
