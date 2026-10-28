//! Runtime-owned compiled import loaders.
//!
//! Generated module metadata needs the loader before `importlib.machinery`
//! exists. These ordinary heap classes and the built-in singleton are the
//! authority shared by that metadata, importlib and the machinery facade.

use super::*;
use crate::builtins::functions::native_callable::NativeCallableKind;

fn loader_class_module(py: &PyToken<'_>, dict_ptr: *mut u8, module: &[u8]) -> bool {
    let ptr = alloc_string(py, module);
    if ptr.is_null() {
        return false;
    }
    let bits = MoltObject::from_ptr(ptr).bits();
    let published = set_class_method(py, dict_ptr, "__module__", bits);
    dec_ref_bits(py, bits);
    published
}

pub(crate) fn compiled_loader_base_class(py: &PyToken<'_>) -> u64 {
    let state = types_state(py);
    init_cached_runtime_class_configured(
        py,
        &state.compiled_loader_base_class,
        "_MoltLoader",
        crate::builtins::types::RuntimeClassLayout {
            semantics: ClassSemanticPolicy::heap(false, true),
            native_module: None,
            layout_size: 8,
            instance_shape: None,
            native_slots: None,
        },
        |class_bits, dict_ptr| {
            let methods = [
                RuntimeClassMethodSpec::with_signature(
                    "create_module",
                    NativeCallableKind::MethodDescriptor,
                    crate::builtins::functions::runtime_fn_addr(
                        "molt_importlib_compiled_loader_create_module",
                        molt_importlib_compiled_loader_create_module as *const (),
                    ),
                    2,
                    RuntimeMethodSignature::new(&[b"self", b"_spec"], false, false),
                ),
                RuntimeClassMethodSpec::with_signature(
                    "exec_module",
                    NativeCallableKind::MethodDescriptor,
                    crate::builtins::functions::runtime_fn_addr(
                        "molt_importlib_compiled_loader_exec_module",
                        molt_importlib_compiled_loader_exec_module as *const (),
                    ),
                    2,
                    RuntimeMethodSignature::new(&[b"self", b"module"], false, false),
                ),
                RuntimeClassMethodSpec::with_signature(
                    "load_module",
                    NativeCallableKind::MethodDescriptor,
                    crate::builtins::functions::runtime_fn_addr(
                        "molt_importlib_compiled_loader_load_module",
                        molt_importlib_compiled_loader_load_module as *const (),
                    ),
                    2,
                    RuntimeMethodSignature::new(&[b"self", b"fullname"], false, false),
                ),
            ];
            loader_class_module(py, dict_ptr, b"importlib.machinery")
                && configure_runtime_class_methods(py, class_bits, dict_ptr, &methods)
        },
    )
}

fn compiled_loader_derived_class(py: &PyToken<'_>, frozen: bool) -> u64 {
    let state = types_state(py);
    let (slot, name) = if frozen {
        (&state.compiled_loader_frozen_class, "FrozenImporter")
    } else {
        (&state.compiled_loader_builtin_class, "BuiltinImporter")
    };
    let base = compiled_loader_base_class(py);
    if base == 0 {
        return 0;
    }
    init_cached_runtime_class_configured(
        py,
        slot,
        name,
        crate::builtins::types::RuntimeClassLayout {
            semantics: ClassSemanticPolicy::heap(false, true),
            native_module: None,
            layout_size: 8,
            instance_shape: None,
            native_slots: None,
        },
        |class_bits, dict_ptr| {
            let set_base = molt_class_set_base(class_bits, base);
            dec_ref_bits(py, set_base);
            if exception_pending(py)
                || class_bases_vec(unsafe {
                    class_bases_bits(obj_from_bits(class_bits).as_ptr().unwrap())
                })
                .as_slice()
                    != [base]
            {
                return false;
            }
            if !loader_class_module(py, dict_ptr, b"_frozen_importlib") {
                return false;
            }
            if frozen {
                return true;
            }
            let none = MoltObject::none().bits();
            let method = crate::builtins::methods::builtin_func_bits_with_signature(
                py,
                crate::builtins::functions::native_callable::NativeCallableSpec::declared(
                    NativeCallableKind::ClassMethodDescriptor,
                    class_bits,
                    "find_spec",
                ),
                crate::builtins::functions::runtime_fn_addr(
                    "molt_importlib_compiled_loader_find_spec",
                    molt_importlib_compiled_loader_find_spec as *const (),
                ),
                4,
                &[none, none],
                0,
                &[b"cls", b"fullname", b"path", b"target"],
                false,
                false,
            );
            set_class_method(py, dict_ptr, "find_spec", method)
        },
    )
}

pub(crate) fn compiled_loader_builtin_class(py: &PyToken<'_>) -> u64 {
    compiled_loader_derived_class(py, false)
}

pub(crate) fn compiled_loader_frozen_class(py: &PyToken<'_>) -> u64 {
    compiled_loader_derived_class(py, true)
}

fn compiled_loader_singleton(py: &PyToken<'_>) -> u64 {
    let state = types_state(py);
    init_atomic_bits(py, &state.compiled_loader_singleton, || {
        let class_bits = compiled_loader_derived_class(py, false);
        if class_bits == 0 {
            return 0;
        }
        let bits = unsafe { call_callable0(py, class_bits) };
        if exception_pending(py) || obj_from_bits(bits).is_none() {
            if !obj_from_bits(bits).is_none() {
                dec_ref_bits(py, bits);
            }
            if !exception_pending(py) {
                let _ =
                    raise_exception::<u64>(py, "MemoryError", "compiled loader allocation failed");
            }
            return 0;
        }
        bits
    })
}

/// A finder over the immutable application catalog. Merely describing a
/// compiled module neither initializes it nor publishes into sys.modules.
/// The descriptor binds subclasses normally, but the spec keeps the existing
/// runtime loader singleton: its create/exec methods are instance descriptors.
/// Discovery must not instantiate a user subclass or invoke its initializer.
#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_compiled_loader_find_spec(
    _class_bits: u64,
    fullname_bits: u64,
    _path_bits: u64,
    _target_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let Some(name) = string_obj_to_owned(obj_from_bits(fullname_bits)) else {
            return raise_exception::<_>(py, "TypeError", "module name must be str");
        };
        let Some(metadata) = crate::builtins::module_table::module_registry()
            .and_then(|registry| registry.compiled_spec(&name))
        else {
            return MoltObject::none().bits();
        };
        // A user source with this same name is not the Molt stdlib provider.
        if metadata.is_stdlib_provider
            && matches!(
                crate::builtins::platform::known_import_absence(py, metadata.provider_name),
                Some(crate::builtins::platform::KnownImportAbsence::Provider(_))
            )
        {
            return MoltObject::none().bits();
        }
        let origin = metadata.origin;
        let is_package = metadata.is_package;
        let origin_ptr = alloc_string(py, origin.as_bytes());
        if origin_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let origin_bits = MoltObject::from_ptr(origin_ptr).bits();
        let loader_bits = compiled_loader_singleton(py);
        if loader_bits == 0 || exception_pending(py) {
            dec_ref_bits(py, origin_bits);
            return MoltObject::none().bits();
        }
        let spec = super::module_spec::alloc_module_spec(
            py,
            fullname_bits,
            loader_bits,
            origin_bits,
            MoltObject::from_bool(is_package).bits(),
        );
        dec_ref_bits(py, origin_bits);
        let Some(spec_bits) = spec else {
            return MoltObject::none().bits();
        };
        if metadata.has_location {
            let Some(key) = attr_name_bits_from_bytes(py, b"has_location") else {
                dec_ref_bits(py, spec_bits);
                return MoltObject::none().bits();
            };
            let located = crate::builtins::attributes::mutate_attr_name(
                py,
                spec_bits,
                key,
                crate::builtins::attr::DescriptorMutation::Set(MoltObject::from_bool(true).bits()),
                molt_cpython_abi::hooks::AttributeMutation::Normal,
            );
            dec_ref_bits(py, key);
            if located.is_err() {
                dec_ref_bits(py, spec_bits);
                return MoltObject::none().bits();
            }
        }
        // A static extension package has a descriptive origin rather than a
        // directory. Leave its canonical empty locations intact; never insert
        // an empty path (which would accidentally authorize cwd discovery).
        if is_package
            && let Some(root) = std::path::Path::new(origin).parent()
            && !root.as_os_str().is_empty()
        {
            let root_text = root.to_string_lossy();
            let root_ptr = alloc_string(py, root_text.as_bytes());
            if root_ptr.is_null() {
                dec_ref_bits(py, spec_bits);
                return MoltObject::none().bits();
            }
            let root_bits = MoltObject::from_ptr(root_ptr).bits();
            let paths = alloc_list(py, &[root_bits]);
            dec_ref_bits(py, root_bits);
            if paths.is_null() {
                dec_ref_bits(py, spec_bits);
                return MoltObject::none().bits();
            }
            let paths_bits = MoltObject::from_ptr(paths).bits();
            let Some(attr) = attr_name_bits_from_bytes(py, b"submodule_search_locations") else {
                dec_ref_bits(py, paths_bits);
                dec_ref_bits(py, spec_bits);
                return MoltObject::none().bits();
            };
            let result = molt_set_attr_name(spec_bits, attr, paths_bits);
            dec_ref_bits(py, attr);
            dec_ref_bits(py, paths_bits);
            dec_ref_bits(py, result);
            if exception_pending(py) {
                dec_ref_bits(py, spec_bits);
                return MoltObject::none().bits();
            }
        }
        spec_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_compiled_loader_types() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let base = compiled_loader_base_class(py);
        let builtin = compiled_loader_derived_class(py, false);
        let frozen = compiled_loader_derived_class(py, true);
        if [base, builtin, frozen].contains(&0) || exception_pending(py) {
            return MoltObject::none().bits();
        }
        let tuple = alloc_tuple(py, &[base, builtin, frozen]);
        if tuple.is_null() {
            return MoltObject::none().bits();
        }
        MoltObject::from_ptr(tuple).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_compiled_loader() -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        crate::state::cache::retain_cached_result(py, compiled_loader_singleton(py))
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_compiled_loader_create_module(
    _self_bits: u64,
    _spec_bits: u64,
) -> u64 {
    MoltObject::none().bits()
}

#[cfg(test)]
fn attr_or_none(py: &PyToken<'_>, target: u64, name: &[u8]) -> Option<u64> {
    let attr = attr_name_bits_from_bytes(py, name)?;
    let bits = molt_getattr_builtin(target, attr, MoltObject::none().bits());
    dec_ref_bits(py, attr);
    if exception_pending(py) {
        if !obj_from_bits(bits).is_none() {
            dec_ref_bits(py, bits);
        }
        None
    } else {
        Some(bits)
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_compiled_loader_exec_module(
    self_bits: u64,
    module_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let none = MoltObject::none().bits();
        let name = crate::molt_importlib_coerce_module_name(module_bits, self_bits, none);
        if exception_pending(py) {
            return none;
        }
        let Some(name_text) = string_obj_to_owned(obj_from_bits(name)) else {
            dec_ref_bits(py, name);
            return raise_exception::<_>(py, "TypeError", "module name must be str");
        };
        let outcome = crate::builtins::modules::execute_compiled_module(
            py,
            &name_text,
            &name_text,
            None,
            false,
            crate::builtins::modules::ExecutionMetadata::LoaderNamespace { module_bits },
        );
        dec_ref_bits(py, name);
        match outcome {
            Ok(executed) => {
                dec_ref_bits(py, executed);
                none
            }
            Err(error) => error.into_import_error(py, &name_text),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_compiled_loader_load_module(
    _self_bits: u64,
    fullname_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let none = MoltObject::none().bits();
        let spec = molt_importlib_compiled_loader_find_spec(_self_bits, fullname_bits, none, none);
        if exception_pending(py) {
            return none;
        }
        if obj_from_bits(spec).is_none() {
            return raise_exception::<_>(
                py,
                "ImportError",
                "module has no compiler-emitted body in this binary",
            );
        }
        let Some(loader_name) = attr_name_bits_from_bytes(py, b"loader") else {
            dec_ref_bits(py, spec);
            return none;
        };
        crate::molt_set_attr_name(spec, loader_name, _self_bits);
        dec_ref_bits(py, loader_name);
        if exception_pending(py) {
            dec_ref_bits(py, spec);
            return none;
        }
        let result = crate::builtins::platform::molt_importlib_load_module_from_spec(
            _self_bits,
            fullname_bits,
            spec,
        );
        molt_cpython_abi::api::errors::with_preserved_error(|| dec_ref_bits(py, spec));
        result
    })
}

#[cfg(test)]
#[path = "compiled_loader/catalog_tests.rs"]
mod catalog_tests;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::object::seq_access::pin_item;

    fn text(py: &PyToken<'_>, value: &[u8]) -> u64 {
        let ptr = alloc_string(py, value);
        assert!(!ptr.is_null());
        MoltObject::from_ptr(ptr).bits()
    }

    fn set_attr(py: &PyToken<'_>, target: u64, name: &[u8], value: u64) {
        let key = text(py, name);
        let result = molt_set_attr_name(target, key, value);
        if !obj_from_bits(result).is_none() {
            dec_ref_bits(py, result);
        }
        dec_ref_bits(py, key);
        assert!(!exception_pending(py));
    }

    fn observed_name(py: &PyToken<'_>, module: u64, loader: u64) -> Option<String> {
        let bits =
            crate::molt_importlib_coerce_module_name(module, loader, MoltObject::none().bits());
        if exception_pending(py) {
            return None;
        }
        let value = string_obj_to_owned(obj_from_bits(bits));
        dec_ref_bits(py, bits);
        value
    }

    fn pending_exception_summary(py: &PyToken<'_>) -> String {
        if !exception_pending(py) {
            return "<none>".to_string();
        }
        let exc = crate::molt_exception_last_pending();
        if obj_from_bits(exc).is_none() {
            return "<pending exception without object>".to_string();
        }
        let kind = crate::molt_exception_kind(exc);
        let message = crate::builtins::exceptions::format_exception_message(
            py,
            obj_from_bits(exc).as_ptr().expect("pending exception"),
        );
        let summary = format!(
            "{}: {}",
            string_obj_to_owned(obj_from_bits(kind)).unwrap_or_else(|| "<kind>".to_string()),
            message
        );
        for bits in [kind, exc] {
            if !obj_from_bits(bits).is_none() {
                dec_ref_bits(py, bits);
            }
        }
        summary
    }

    /// Publish `sys` through the production cache-set bootstrap. Bare runtime
    /// unit tests have no linked compiled `sys` initializer.
    struct SysFixture {
        name: u64,
        module: u64,
        installed: bool,
    }

    impl SysFixture {
        fn new(py: &PyToken<'_>) -> Self {
            let name = text(py, b"sys");
            let cached = crate::molt_module_cache_get(name);
            assert!(!exception_pending(py), "{}", pending_exception_summary(py));
            if !obj_from_bits(cached).is_none() {
                crate::builtins::module_table::publish_interpreter_sys_for_test(py, cached);
                return Self {
                    name,
                    module: cached,
                    installed: false,
                };
            }
            let module = crate::molt_module_new(name);
            assert!(!exception_pending(py), "{}", pending_exception_summary(py));
            assert!(!obj_from_bits(module).is_none());
            let publication =
                crate::builtins::module_table::publish_interpreter_sys_for_test(py, module);
            assert!(obj_from_bits(publication).is_none());
            assert!(!exception_pending(py), "{}", pending_exception_summary(py));
            Self {
                name,
                module,
                installed: true,
            }
        }
    }

    impl Drop for SysFixture {
        fn drop(&mut self) {
            crate::with_gil_entry_nopanic!(py, {
                if self.installed {
                    clear_exception(py);
                    let removed = crate::molt_module_cache_del(self.name);
                    if !obj_from_bits(removed).is_none() {
                        dec_ref_bits(py, removed);
                    }
                    clear_exception(py);
                }
                dec_ref_bits(py, self.module);
                dec_ref_bits(py, self.name);
            });
        }
    }

    #[test]
    fn facade_payload_and_generated_loader_share_mutable_heap_types() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let base = compiled_loader_base_class(py);
            let builtin = compiled_loader_builtin_class(py);
            let frozen = compiled_loader_derived_class(py, true);
            assert_ne!(base, 0);
            assert_eq!(
                class_bases_vec(unsafe {
                    class_bases_bits(obj_from_bits(builtin).as_ptr().unwrap())
                }),
                vec![base]
            );
            assert_eq!(
                class_bases_vec(unsafe {
                    class_bases_bits(obj_from_bits(frozen).as_ptr().unwrap())
                }),
                vec![base]
            );
            let payload = molt_importlib_compiled_loader_types();
            let tuple_ptr = obj_from_bits(payload).as_ptr().expect("loader types tuple");
            for (index, expected) in [base, builtin, frozen].into_iter().enumerate() {
                let observed =
                    unsafe { pin_item(py, tuple_ptr, index) }.expect("loader class tuple item");
                assert_eq!(observed.bits(), expected);
            }
            let frozen_payload = crate::molt_importlib_frozen_payload();
            assert!(!exception_pending(py));
            let frozen_ptr = obj_from_bits(frozen_payload)
                .as_ptr()
                .expect("frozen payload dict");
            let module_spec = crate::molt_importlib_module_spec_type();
            for (name, expected) in [
                (b"BuiltinImporter".as_slice(), builtin),
                (b"FrozenImporter".as_slice(), frozen),
                (b"ModuleSpec".as_slice(), module_spec),
            ] {
                let key = text(py, name);
                assert_eq!(
                    unsafe {
                        dict_get_in_place(py, frozen_ptr, key).expect("dictionary lookup succeeds")
                    },
                    Some(expected)
                );
                dec_ref_bits(py, key);
            }
            let loader = molt_importlib_compiled_loader();
            let again = molt_importlib_compiled_loader();
            assert_eq!(loader, again);
            assert_eq!(type_of_bits(py, loader), builtin);
            // Public importers inherit ordinary object representation, as in
            // CPython; no loader-local dunder may replace that shared protocol.
            let repr_name = text(py, b"__repr__");
            for class in [base, builtin, frozen] {
                let class_ptr = obj_from_bits(class).as_ptr().expect("loader class");
                let dict_ptr = obj_from_bits(unsafe { class_dict_bits(class_ptr) })
                    .as_ptr()
                    .expect("loader class dict");
                assert!(
                    unsafe {
                        dict_get_in_place(py, dict_ptr, repr_name)
                            .expect("dictionary lookup succeeds")
                    }
                    .is_none()
                );
            }
            dec_ref_bits(py, repr_name);
            let create = attr_or_none(py, loader, b"create_module").expect("bound method");
            let created = unsafe { call_callable1(py, create, MoltObject::none().bits()) };
            assert!(!exception_pending(py));
            assert!(obj_from_bits(created).is_none());
            let marker = MoltObject::from_int(41).bits();
            set_attr(py, builtin, b"runtime_loader_marker", marker);
            let inherited = attr_or_none(py, loader, b"runtime_loader_marker")
                .expect("mutable class attribute inherited by singleton");
            assert_eq!(inherited, marker);
            let marker_name = text(py, b"runtime_loader_marker");
            let removed = crate::molt_del_attr_name(builtin, marker_name);
            assert!(obj_from_bits(removed).is_none());
            assert!(!exception_pending(py));
            dec_ref_bits(py, marker_name);
            for bits in [create, again, loader, module_spec, frozen_payload, payload] {
                if !obj_from_bits(bits).is_none() {
                    dec_ref_bits(py, bits);
                }
            }
        });
    }

    #[test]
    fn compiled_loader_uses_runtime_name_coercion_fallbacks() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let class = compiled_loader_builtin_class(py);
            let loader = unsafe { call_callable0(py, class) };
            assert!(!exception_pending(py));
            let name = text(py, b"original");
            let module = crate::molt_module_new(name);
            assert!(!exception_pending(py));
            let invalid = MoltObject::from_int(17).bits();
            set_attr(py, module, b"__name__", invalid);

            let spec_name = text(py, b"from.spec");
            let spec = alloc_module_spec(
                py,
                spec_name,
                loader,
                MoltObject::none().bits(),
                MoltObject::none().bits(),
            )
            .expect("runtime spec");
            set_attr(py, module, b"__spec__", spec);
            let resources_name =
                crate::molt_importlib_resources_module_name(module, MoltObject::none().bits());
            assert!(!exception_pending(py), "{}", pending_exception_summary(py));
            assert_eq!(
                string_obj_to_owned(obj_from_bits(resources_name)).as_deref(),
                Some("from.spec")
            );
            dec_ref_bits(py, resources_name);
            assert_eq!(
                observed_name(py, module, loader).as_deref(),
                Some("from.spec")
            );

            let rejected =
                crate::molt_importlib_coerce_module_name(MoltObject::none().bits(), loader, spec);
            assert!(obj_from_bits(rejected).is_none());
            assert!(exception_pending(py));
            let setter_exc = crate::molt_exception_last_pending();
            let setter_kind = crate::molt_exception_kind(setter_exc);
            assert_eq!(
                string_obj_to_owned(obj_from_bits(setter_kind)).as_deref(),
                Some("AttributeError")
            );
            clear_exception(py);
            dec_ref_bits(py, setter_kind);
            dec_ref_bits(py, setter_exc);

            set_attr(py, module, b"__name__", invalid);
            set_attr(py, module, b"__spec__", MoltObject::none().bits());
            let loader_name = text(py, b"from.loader");
            set_attr(py, loader, b"name", loader_name);
            assert_eq!(
                observed_name(py, module, loader).as_deref(),
                Some("from.loader")
            );

            set_attr(py, module, b"__name__", invalid);
            set_attr(py, loader, b"name", invalid);
            assert_eq!(observed_name(py, module, loader), None);
            assert!(exception_pending(py));
            let exc = crate::molt_exception_last_pending();
            let kind = crate::molt_exception_kind(exc);
            assert_eq!(
                string_obj_to_owned(obj_from_bits(kind)).as_deref(),
                Some("TypeError")
            );
            clear_exception(py);
            for bits in [
                exc,
                kind,
                loader_name,
                spec,
                spec_name,
                module,
                name,
                loader,
            ] {
                if !obj_from_bits(bits).is_none() {
                    dec_ref_bits(py, bits);
                }
            }
        });
    }

    #[test]
    fn load_module_rejects_invalid_name_without_mutating_public_entry() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let _sys = SysFixture::new(py);
            let loader = molt_importlib_compiled_loader();
            let modules =
                crate::builtins::platform::importlib_runtime_modules_bits(py).expect("sys.modules");
            let modules_ptr = obj_from_bits(modules).as_ptr().expect("modules dict");
            let invalid_name = MoltObject::from_int(934_771).bits();
            let stale_name = text(py, b"displaced_loader_stale");
            let stale = crate::molt_module_new(stale_name);
            assert!(!exception_pending(py));
            unsafe { assert!(dict_set_in_place(py, modules_ptr, invalid_name, stale)) };
            assert!(!exception_pending(py));
            let failed = molt_importlib_compiled_loader_load_module(loader, invalid_name);
            assert!(obj_from_bits(failed).is_none());
            assert!(exception_pending(py));
            let exc = crate::molt_exception_last_pending();
            let kind = crate::molt_exception_kind(exc);
            assert_eq!(
                string_obj_to_owned(obj_from_bits(kind)).as_deref(),
                Some("TypeError")
            );
            clear_exception(py);
            assert_eq!(
                unsafe {
                    dict_get_in_place(py, modules_ptr, invalid_name)
                        .expect("dictionary lookup succeeds")
                },
                Some(stale)
            );
            let _ = unsafe { dict_del_in_place(py, modules_ptr, invalid_name) };
            assert!(!exception_pending(py));
            for bits in [kind, exc, stale, stale_name, modules, loader] {
                if !obj_from_bits(bits).is_none() {
                    dec_ref_bits(py, bits);
                }
            }
        });
    }

    #[test]
    fn exec_module_requires_compiled_body_without_copying_public_module() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let sys = SysFixture::new(py);
            let loader = molt_importlib_compiled_loader();
            let target_name = text(py, b"compiled_loader_copy_target");
            let target = crate::molt_module_new(target_name);
            assert!(!exception_pending(py));
            let modules =
                crate::builtins::platform::importlib_runtime_modules_bits(py).expect("sys.modules");
            let modules_ptr = obj_from_bits(modules).as_ptr().unwrap();
            assert!(unsafe { dict_set_in_place(py, modules_ptr, target_name, sys.module) });
            let outcome = molt_importlib_compiled_loader_exec_module(loader, target);
            assert!(obj_from_bits(outcome).is_none());
            assert!(exception_pending(py));
            assert!(pending_exception_summary(py).contains("no compiler-emitted body"));
            clear_exception(py);
            let visible = unsafe { dict_get_in_place(py, modules_ptr, target_name) }
                .unwrap()
                .unwrap();
            assert_ne!(
                visible, target,
                "direct exec never adopts or publishes a public cached module"
            );
            assert!(unsafe { dict_del_in_place(py, modules_ptr, target_name) }.unwrap());
            for bits in [modules, target, target_name, loader] {
                if !obj_from_bits(bits).is_none() {
                    dec_ref_bits(py, bits);
                }
            }
        });
    }
}
