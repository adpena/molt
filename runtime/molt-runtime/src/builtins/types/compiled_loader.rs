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
        if !exception_pending(py) {
            let _ = raise_exception::<u64>(py, "MemoryError", "loader module allocation failed");
        }
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
        ClassSemanticPolicy::heap(false, true),
        8,
        None,
        None,
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
        ClassSemanticPolicy::heap(false, true),
        8,
        None,
        None,
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
            loader_class_module(py, dict_ptr, b"_frozen_importlib")
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
            if !exception_pending(py) {
                return raise_exception::<_>(
                    py,
                    "MemoryError",
                    "loader types tuple allocation failed",
                );
            }
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

fn attr_or_none(py: &PyToken<'_>, target: u64, name: &[u8]) -> Option<u64> {
    let Some(attr) = attr_name_bits_from_bytes(py, name) else {
        if !exception_pending(py) {
            let _ = raise_exception::<u64>(
                py,
                "MemoryError",
                "loader attribute name allocation failed",
            );
        }
        return None;
    };
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

fn required_attr(py: &PyToken<'_>, target: u64, name: &[u8]) -> Option<u64> {
    let Some(attr) = attr_name_bits_from_bytes(py, name) else {
        if !exception_pending(py) {
            let _ = raise_exception::<u64>(
                py,
                "MemoryError",
                "loader attribute name allocation failed",
            );
        }
        return None;
    };
    let bits = crate::molt_get_attr_name(target, attr);
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

/// The old machinery loader temporarily evicted a different visible module
/// so `molt_module_import` executes the compiled body instead of returning
/// that stale entry. Keep the displaced object owned until success or restore.
struct DisplacedModule<'a, 'py> {
    py: &'a PyToken<'py>,
    modules: u64,
    name: u64,
    previous: Option<u64>,
}

impl<'a, 'py> DisplacedModule<'a, 'py> {
    fn begin(py: &'a PyToken<'py>, name: u64, current: Option<u64>) -> Option<Self> {
        let modules = match crate::builtins::platform::importlib_runtime_modules_bits(py) {
            Ok(bits) => bits,
            Err(_) => return None,
        };
        let Some(dict_ptr) = obj_from_bits(modules).as_ptr() else {
            dec_ref_bits(py, modules);
            let _ = raise_exception::<u64>(py, "RuntimeError", "sys.modules is unavailable");
            return None;
        };
        let existing = unsafe { dict_get_in_place(py, dict_ptr, name) };
        if exception_pending(py) {
            dec_ref_bits(py, modules);
            return None;
        }
        let previous =
            existing.filter(|bits| !obj_from_bits(*bits).is_none() && current != Some(*bits));
        if let Some(bits) = previous {
            inc_ref_bits(py, bits);
            let _ = unsafe { dict_del_in_place(py, dict_ptr, name) };
            if exception_pending(py) {
                dec_ref_bits(py, bits);
                dec_ref_bits(py, modules);
                return None;
            }
        }
        inc_ref_bits(py, name);
        Some(Self {
            py,
            modules,
            name,
            previous,
        })
    }

    fn restore_on_error(&mut self) {
        let Some(previous) = self.previous.take() else {
            return;
        };
        let saved = crate::molt_exception_last_pending();
        clear_exception(self.py);
        if let Some(dict_ptr) = obj_from_bits(self.modules).as_ptr() {
            let present = unsafe { dict_get_in_place(self.py, dict_ptr, self.name) };
            if present.is_none() && !exception_pending(self.py) {
                unsafe { dict_set_in_place(self.py, dict_ptr, self.name, previous) };
            }
        }
        if !exception_pending(self.py) && !obj_from_bits(saved).is_none() {
            let _ = crate::molt_exception_set_last(saved);
        }
        if !obj_from_bits(saved).is_none() {
            dec_ref_bits(self.py, saved);
        }
        dec_ref_bits(self.py, previous);
    }
}

impl Drop for DisplacedModule<'_, '_> {
    fn drop(&mut self) {
        if let Some(bits) = self.previous.take() {
            dec_ref_bits(self.py, bits);
        }
        dec_ref_bits(self.py, self.name);
        dec_ref_bits(self.py, self.modules);
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
        let Some(mut displaced) = DisplacedModule::begin(py, name, Some(module_bits)) else {
            dec_ref_bits(py, name);
            return none;
        };
        let imported = crate::molt_module_import(name);
        dec_ref_bits(py, name);
        if exception_pending(py) {
            if !obj_from_bits(imported).is_none() {
                dec_ref_bits(py, imported);
            }
            displaced.restore_on_error();
            return none;
        }
        let payload = if isinstance_runtime(py, imported, builtin_classes(py).dict) {
            inc_ref_bits(py, imported);
            imported
        } else {
            match attr_or_none(py, imported, b"__dict__") {
                Some(bits) if isinstance_runtime(py, bits, builtin_classes(py).dict) => bits,
                Some(bits) => {
                    if !obj_from_bits(bits).is_none() {
                        dec_ref_bits(py, bits);
                    }
                    let message = format!(
                        "import returned non-module payload: {}",
                        type_name(py, obj_from_bits(imported))
                    );
                    let _ = raise_exception::<u64>(py, "TypeError", &message);
                    none
                }
                None => none,
            }
        };
        if !exception_pending(py) {
            if let Some(target) = required_attr(py, module_bits, b"__dict__") {
                if let Some(update) = required_attr(py, target, b"update") {
                    let out = unsafe { call_callable1(py, update, payload) };
                    if !obj_from_bits(out).is_none() {
                        dec_ref_bits(py, out);
                    }
                    dec_ref_bits(py, update);
                }
                dec_ref_bits(py, target);
            }
        }
        if !obj_from_bits(payload).is_none() {
            dec_ref_bits(py, payload);
        }
        if !obj_from_bits(imported).is_none() {
            dec_ref_bits(py, imported);
        }
        if exception_pending(py) {
            displaced.restore_on_error();
        }
        none
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_compiled_loader_load_module(
    _self_bits: u64,
    fullname_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let none = MoltObject::none().bits();
        let Some(mut displaced) = DisplacedModule::begin(py, fullname_bits, None) else {
            return none;
        };
        let imported = crate::molt_module_import(fullname_bits);
        if exception_pending(py) {
            if !obj_from_bits(imported).is_none() {
                dec_ref_bits(py, imported);
            }
            displaced.restore_on_error();
            return none;
        }
        imported
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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
                    unsafe { dict_get_in_place(py, frozen_ptr, key) },
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
                assert!(unsafe { dict_get_in_place(py, dict_ptr, repr_name) }.is_none());
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
    fn load_module_returns_the_runtime_module_and_restores_a_displaced_entry_on_error() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let sys = SysFixture::new(py);
            let loader = molt_importlib_compiled_loader();
            let sys_name = sys.name;
            let imported = molt_importlib_compiled_loader_load_module(loader, sys_name);
            assert!(!exception_pending(py), "{}", pending_exception_summary(py));
            assert_eq!(imported, sys.module);
            let observed_name = attr_or_none(py, imported, b"__name__").expect("sys name");
            assert_eq!(
                string_obj_to_owned(obj_from_bits(observed_name)).as_deref(),
                Some("sys")
            );

            let modules =
                crate::builtins::platform::importlib_runtime_modules_bits(py).expect("sys.modules");
            let modules_ptr = obj_from_bits(modules).as_ptr().expect("modules dict");
            let invalid_name = MoltObject::from_int(934_771).bits();
            let stale_name = text(py, b"displaced_loader_stale");
            let stale = crate::molt_module_new(stale_name);
            assert!(!exception_pending(py));
            unsafe { dict_set_in_place(py, modules_ptr, invalid_name, stale) };
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
                unsafe { dict_get_in_place(py, modules_ptr, invalid_name) },
                Some(stale)
            );
            let _ = unsafe { dict_del_in_place(py, modules_ptr, invalid_name) };
            assert!(!exception_pending(py));
            for bits in [
                kind,
                exc,
                stale,
                stale_name,
                modules,
                observed_name,
                imported,
                loader,
            ] {
                if !obj_from_bits(bits).is_none() {
                    dec_ref_bits(py, bits);
                }
            }
        });
    }

    #[test]
    fn exec_module_copies_imported_module_dict_into_supplied_module() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let sys = SysFixture::new(py);
            let loader = molt_importlib_compiled_loader();
            let target_name = text(py, b"compiled_loader_copy_target");
            let target = crate::molt_module_new(target_name);
            assert!(!exception_pending(py));
            let sys_name = sys.name;
            set_attr(py, target, b"__name__", sys_name);
            let outcome = molt_importlib_compiled_loader_exec_module(loader, target);
            assert!(obj_from_bits(outcome).is_none());
            assert!(!exception_pending(py), "{}", pending_exception_summary(py));
            let copied_modules = attr_or_none(py, target, b"modules").expect("copied sys.modules");
            assert!(isinstance_runtime(
                py,
                copied_modules,
                builtin_classes(py).dict
            ));
            let modules =
                crate::builtins::platform::importlib_runtime_modules_bits(py).expect("sys.modules");
            let modules_ptr = obj_from_bits(modules).as_ptr().expect("modules dict");
            let visible =
                unsafe { dict_get_in_place(py, modules_ptr, sys_name) }.expect("visible sys");
            assert_ne!(
                visible, target,
                "direct exec_module must not publish its target"
            );
            for bits in [modules, copied_modules, target, target_name, loader] {
                if !obj_from_bits(bits).is_none() {
                    dec_ref_bits(py, bits);
                }
            }
        });
    }
}
