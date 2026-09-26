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
    let result = {
        let _context = PackageContext::enter(name);
        unsafe { init() }
    };
    // This boundary is indivisible to generated-code exception checks. A C
    // result with an error is always validated and released, never abandoned.
    initialize_result(py, result, name_bits, origin_bits, spec_bits, create_only)
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

struct ModulePublication {
    object: *mut PyObject,
    name_bits: u64,
    def: *mut PyModuleDef,
    register_state: bool,
    published: bool,
    committed: bool,
}

impl ModulePublication {
    fn publish(&mut self, py: &crate::PyToken<'_>, name: &str, bits: u64) -> Result<(), String> {
        let existing = crate::builtins::modules::molt_module_cache_get(self.name_bits);
        if crate::exception_pending(py) {
            return Err(String::new());
        }
        if !MoltObject::from_bits(existing).is_none() {
            dec_ref_bits(py, existing);
            return Err(format!(
                "{name}: extension publication found an existing module"
            ));
        }
        // Cache set may fail after installing its runtime owner; from this
        // point the rollback owns that partially published transaction too.
        self.published = true;
        let _ = crate::builtins::modules::molt_module_cache_set(self.name_bits, bits);
        if crate::exception_pending(py) {
            return Err(String::new());
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
        if self.register_state && crate::c_api::molt_module_state_add(bits, self.def.addr()) != 0 {
            with_preserved_native_error(|| unsafe { hook_dec_ref(bits) });
            return Err("extension module state registration failed".into());
        }
        self.committed = true;
        Ok(bits)
    }
}

impl Drop for ModulePublication {
    fn drop(&mut self) {
        let pending = take_native_pending_snapshot();
        if !self.committed {
            if self.published {
                let current = crate::builtins::modules::molt_module_cache_get(self.name_bits);
                let own = molt_cpython_abi::bridge::GLOBAL_BRIDGE
                    .molt_handle_for_pyobj(self.object)
                    .map(|value| value.bits());
                if own == Some(current) {
                    let _ = crate::builtins::modules::molt_module_cache_del(self.name_bits);
                }
                unsafe { hook_dec_ref(current) };
            }
        }
        unsafe { molt_cpython_abi::api::refcount::Py_DECREF(self.object) };
        restore_native_pending_snapshot(pending);
    }
}

unsafe fn module_result_to_bits(
    py: &crate::PyToken<'_>,
    object: *mut PyObject,
    def: *mut PyModuleDef,
    name_bits: u64,
    name: &str,
    spec: *mut PyObject,
    origin_bits: u64,
    create_only: bool,
    single_phase: bool,
) -> Result<u64, String> {
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
        with_preserved_native_error(|| unsafe {
            molt_cpython_abi::api::refcount::Py_DECREF(value)
        });
        if status != 0 {
            return Err(format!(
                "{name}: extension {} metadata initialization failed",
                destination.to_string_lossy()
            ));
        }
    }
    if !create_only {
        publication.publish(py, name, bits)?;
        if !def.is_null()
            && unsafe { molt_cpython_abi::api::modules::PyModule_ExecDef(object, def) } != 0
        {
            return Err(format!(
                "{name}: PyModuleDef Py_mod_exec slot returned non-zero"
            ));
        }
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

fn initialize_result(
    py: &crate::PyToken<'_>,
    result: *mut PyObject,
    name_bits: u64,
    origin_bits: u64,
    supplied_spec: u64,
    create_only: bool,
) -> u64 {
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
            let pending = take_native_pending_snapshot();
            unsafe { molt_cpython_abi::api::refcount::Py_DECREF(result) };
            restore_native_pending_snapshot(pending);
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
            with_preserved_native_error(|| unsafe {
                molt_cpython_abi::api::refcount::Py_DECREF(result)
            });
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
    let spec = if MoltObject::from_bits(supplied_spec).is_none() {
        unsafe { module_spec(py, name_bits, origin_bits) }
    } else {
        let ptr = unsafe {
            molt_cpython_abi::bridge::GLOBAL_BRIDGE.borrowed_handle_to_new_pyobj(supplied_spec)
        };
        (!ptr.is_null()).then_some(ptr)
    };
    let Some(spec) = spec else {
        if !definition {
            with_preserved_native_error(|| unsafe {
                molt_cpython_abi::api::refcount::Py_DECREF(result)
            });
        }
        return initialization_failure(
            py,
            &format!("{name}: PyModuleDef ModuleSpec bridge failed"),
        );
    };
    let module = if definition {
        if unsafe { (*def).m_name.is_null() || CStr::from_ptr((*def).m_name).to_bytes().is_empty() }
        {
            with_preserved_native_error(|| unsafe {
                molt_cpython_abi::api::refcount::Py_DECREF(spec)
            });
            return initialization_failure(
                py,
                "extension PyInit returned an invalid module definition",
            );
        }
        let module =
            unsafe { molt_cpython_abi::api::modules::PyModule_FromDefAndSpec2(def, spec, 0) };
        if module.is_null() {
            with_preserved_native_error(|| unsafe {
                molt_cpython_abi::api::refcount::Py_DECREF(spec)
            });
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
            name_bits,
            &name,
            spec,
            origin_bits,
            create_only,
            !definition,
        )
    };
    with_preserved_native_error(|| unsafe { molt_cpython_abi::api::refcount::Py_DECREF(spec) });
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
            module_name_bits,
            MoltObject::none().bits(),
            MoltObject::none().bits(),
            false,
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
pub(crate) fn execute_prepared_extension(
    module_bits: u64,
    name_bits: u64,
    publish_cache: bool,
) -> u64 {
    with_gil(|py| unsafe {
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
        if crate::c_api::module_exec_started(module_bits) {
            return MoltObject::none().bits();
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
        let existing = crate::builtins::modules::molt_module_cache_get(name_bits);
        if crate::exception_pending(&py) {
            return MoltObject::none().bits();
        }
        if existing != module_bits && !MoltObject::from_bits(existing).is_none() {
            dec_ref_bits(&py, existing);
            return crate::raise_exception::<u64>(
                &py,
                "ImportError",
                "extension exec module differs from the published identity",
            );
        }
        if existing == module_bits {
            dec_ref_bits(&py, existing);
        } else if let Err(message) = publication.publish(&py, &name, module_bits) {
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
                    name_bits,
                    MoltObject::none().bits(),
                    spec_bits,
                    true,
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
