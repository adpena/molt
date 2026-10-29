use super::*;
use std::collections::BTreeMap;
#[cfg(feature = "stdlib_archive")]
use std::io::Write;

#[test]
fn pending_error_filters_preserve_independent_handled_error() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let outer = alloc_exception(py, "RuntimeError", "handled owner");
        let outer_bits = MoltObject::from_ptr(outer).bits();
        exception_stack_push();
        crate::builtins::exceptions::exception_context_set(py, outer_bits);
        for kind in ["AttributeError", "TypeError", "ImportError", "OSError"] {
            let inner = alloc_exception(py, kind, "raised owner");
            let inner_bits = MoltObject::from_ptr(inner).bits();
            record_exception(py, inner);
            // Matching the handled error must neither match nor consume raised state.
            assert!(!clear_pending_if_kind(py, &["RuntimeError"]));
            assert!(exception_pending(py));
            let observed = molt_exception_last_pending();
            assert_eq!(observed, inner_bits);
            dec_ref_bits(py, observed);
            assert!(clear_pending_if_kind(py, &[kind]));
            assert!(!exception_pending(py));
            assert_eq!(
                crate::builtins::exceptions::exception_context_active_bits(),
                Some(outer_bits)
            );
            dec_ref_bits(py, inner_bits);
        }
        let instance = molt_object_new_bound(builtin_classes(py).object);
        let missing_name = alloc_test_string_bits(py, "unpublished_optional_attribute");
        assert_eq!(getattr_optional_bits(py, instance, missing_name), Ok(None));
        assert!(!exception_pending(py));
        assert_eq!(
            crate::builtins::exceptions::exception_context_active_bits(),
            Some(outer_bits)
        );
        dec_ref_bits(py, missing_name);
        dec_ref_bits(py, instance);
        crate::builtins::exceptions::exception_context_set(py, MoltObject::none().bits());
        exception_stack_pop(py);
        dec_ref_bits(py, outer_bits);
    });
}

#[test]
fn dynamic_extension_admission_reports_complete_static_grants_before_path_access() {
    for (grants, missing) in [
        ("", "fs.read, module.extension.exec"),
        ("fs.read", "module.extension.exec"),
        ("module.extension.exec", "fs.read"),
    ] {
        crate::test_support::RuntimeTestTransaction::with_explicit_capabilities(grants, |py| {
            let name = alloc_test_string_bits(py, "capability_refusal_not_a_compiled_module");
            // Invalid path: a filesystem check would produce ImportError, so
            // PermissionError proves admission preceded that protected action.
            let path = alloc_test_string_bits(py, "");
            let result = molt_importlib_extension_loader_create_module(
                name,
                path,
                MoltObject::none().bits(),
            );
            assert_eq!(result, MoltObject::none().bits());
            let error = crate::molt_exception_last_pending();
            let _owner = unsafe {
                molt_runtime_core::OwnedRuntimeValue::from_owned_bits(py.core_token(), error)
            };
            assert!(crate::builtins::exceptions::exception_matches_builtin_name(
                py,
                error,
                "PermissionError"
            ));
            crate::molt_exception_clear();
            assert_eq!(
                crate::builtins::exceptions::format_exception_message(
                    py,
                    obj_from_bits(error).as_ptr().unwrap()
                ),
                format!("missing [{missing}] capabilities for 'importlib.exec.extension.module'")
            );
            dec_ref_bits(py, name);
            dec_ref_bits(py, path);
        });
    }
}

fn with_env_state<R>(entries: &[(&str, &str)], f: impl FnOnce() -> R) -> R {
    crate::test_support::RuntimeTestTransaction::with_cold_runtime_lifecycle(|| {
        with_env_projection(entries, f)
    })
}

fn with_env_projection<R>(entries: &[(&str, &str)], f: impl FnOnce() -> R) -> R {
    struct Restore(Vec<(String, Option<std::ffi::OsString>)>);
    impl Drop for Restore {
        fn drop(&mut self) {
            for (key, value) in &self.0 {
                process_env_set(std::ffi::OsStr::new(key), value.as_deref())
                    .expect("restore startup input");
            }
        }
    }
    let _restore = Restore(
        entries
            .iter()
            .map(|(key, _)| ((*key).to_owned(), std::env::var_os(key)))
            .collect(),
    );
    for (key, value) in entries {
        process_env_set(std::ffi::OsStr::new(key), Some(std::ffi::OsStr::new(value)))
            .expect("set startup input");
    }
    f()
}

fn platform_test_path(parts: &[&str]) -> String {
    let sep = bootstrap_path_sep();
    let path = if sep == '\\' {
        "C:\\tmp".to_string()
    } else {
        "/tmp".to_string()
    };
    path_join_many_text(path, parts, sep)
}

#[test]
fn importlib_spec_exec_publication_custody_is_exact_and_scoped() {
    assert!(!importlib_spec_exec_owns(0x11));
    {
        let _outer = ImportlibSpecExecTargetGuard::enter(0x11);
        assert!(importlib_spec_exec_owns(0x11));
        assert!(!importlib_spec_exec_owns(0x22));
        {
            let _inner = ImportlibSpecExecTargetGuard::enter(0x22);
            assert!(!importlib_spec_exec_owns(0x11));
            assert!(importlib_spec_exec_owns(0x22));
        }
        assert!(importlib_spec_exec_owns(0x11));
    }
    assert!(!importlib_spec_exec_owns(0x11));
}

#[test]
fn importlib_system_module_authority_returns_and_releases_one_owner() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let name_ptr = alloc_string(_py, b"sys");
        assert!(!name_ptr.is_null());
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let module_ptr = crate::object::builders::alloc_module_obj(_py, name_bits);
        dec_ref_bits(_py, name_bits);
        assert!(!module_ptr.is_null());
        let module_bits = MoltObject::from_ptr(module_ptr).bits();
        let refcount = || unsafe { (*crate::header_from_obj_ptr(module_ptr)).ref_count_snapshot() };
        let prior = crate::builtins::module_table::replace_module_owned_for_test(_py, "sys", None);
        crate::builtins::module_table::publish_interpreter_sys_for_test(_py, module_bits);
        let baseline = refcount();
        {
            let sys = importlib_system_module(_py).expect("interpreter sys module");
            assert_eq!(sys.bits(), module_bits);
            assert_eq!(refcount(), baseline + 1, "resolver returns one owner");
        }
        assert_eq!(refcount(), baseline, "resolver releases its owner");
        let name = crate::attr_name_bits_from_bytes(_py, b"sys").unwrap();
        crate::builtins::modules::molt_module_cache_del(name);
        dec_ref_bits(_py, name);
        if let Some(prior) = prior {
            let displaced = crate::builtins::module_table::replace_module_owned_for_test(
                _py,
                "sys",
                Some(prior),
            );
            assert!(displaced.is_none());
        }

        dec_ref_bits(_py, module_bits);
    });
}

#[test]
fn import_attempt_reports_a_resolver_miss_without_manufacturing_an_exception() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        let _resolver = crate::test_support::EmptyImportResolver::new(_py);
        let name = attr_name_bits_from_bytes(_py, b"molt_absent_import_outcome_probe").unwrap();
        let outcome = crate::builtins::modules::module_import_attempt(name);
        assert!(matches!(
            outcome,
            Ok(crate::builtins::modules::ModuleImportOutcome::Missing { .. })
        ));
        assert!(!exception_pending(_py));
        dec_ref_bits(_py, name);
    });
}

fn bootstrap_module_file() -> String {
    if sys_platform_str().starts_with("win") {
        "C:\\repo\\src\\molt\\stdlib\\sys.py".to_string()
    } else {
        "/repo/src/molt/stdlib/sys.py".to_string()
    }
}

fn bootstrap_stdlib_submodule_file() -> String {
    if sys_platform_str().starts_with("win") {
        "C:\\repo\\src\\molt\\stdlib\\importlib\\util.py".to_string()
    } else {
        "/repo/src/molt/stdlib/importlib/util.py".to_string()
    }
}

fn expected_stdlib_root() -> String {
    if sys_platform_str().starts_with("win") {
        "C:\\repo\\src\\molt\\stdlib".to_string()
    } else {
        "/repo/src/molt/stdlib".to_string()
    }
}

fn extension_boundary_temp_dir(prefix: &str) -> std::path::PathBuf {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    std::env::temp_dir().join(format!("{prefix}_{}_{}", std::process::id(), stamp))
}

fn extension_boundary_filename() -> &'static str {
    if sys_platform_str().starts_with("win") {
        "native.pyd"
    } else {
        "native.so"
    }
}

fn extension_boundary_module_filename(module_basename: &str) -> String {
    if sys_platform_str().starts_with("win") {
        format!("{module_basename}.pyd")
    } else {
        format!("{module_basename}.so")
    }
}

fn clear_extension_metadata_validation_cache() {
    let cache = extension_metadata_ok_cache();
    let mut guard = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.clear();
    EXTENSION_METADATA_CACHE_HITS.store(0, Ordering::Relaxed);
    EXTENSION_METADATA_CACHE_MISSES.store(0, Ordering::Relaxed);
}

#[test]
fn platform_constant_runtime_state_is_owned_and_clearable() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(_py, {
        platform_clear_runtime_state(_py, runtime_state(_py));
    });

    let os_name_bits = molt_os_name();
    let sys_platform_bits = molt_sys_platform();
    let errno_constants_bits = molt_errno_constants();
    let socket_constants_bits = molt_socket_constants();
    assert!(!obj_from_bits(os_name_bits).is_none());
    assert!(!obj_from_bits(sys_platform_bits).is_none());
    assert!(!obj_from_bits(errno_constants_bits).is_none());
    assert!(!obj_from_bits(socket_constants_bits).is_none());

    crate::with_gil_entry_nopanic!(_py, {
        dec_ref_bits(_py, os_name_bits);
        dec_ref_bits(_py, sys_platform_bits);
        dec_ref_bits(_py, errno_constants_bits);
        dec_ref_bits(_py, socket_constants_bits);
        let state = runtime_state(_py);
        for slot in state.platform.object_slots() {
            assert_ne!(slot.load(Ordering::Acquire), 0);
        }
        platform_clear_runtime_state(_py, state);
        for slot in state.platform.object_slots() {
            assert_eq!(slot.load(Ordering::Acquire), 0);
        }
    });
}

#[test]
fn platform_constant_cache_returns_owned_values() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();

    crate::with_gil_entry_nopanic!(_py, {
        let state = runtime_state(_py);
        platform_clear_runtime_state(_py, state);
        for make_bits in [
            molt_os_name as extern "C" fn() -> u64,
            molt_sys_platform as extern "C" fn() -> u64,
            molt_errno_constants as extern "C" fn() -> u64,
            molt_socket_constants as extern "C" fn() -> u64,
        ] {
            let first = make_bits();
            assert!(!obj_from_bits(first).is_none());
            dec_ref_bits(_py, first);

            let second = make_bits();
            assert!(!obj_from_bits(second).is_none());
            dec_ref_bits(_py, second);
        }
        platform_clear_runtime_state(_py, state);
    });
}

fn alloc_test_string_bits(_py: &PyToken<'_>, value: &str) -> u64 {
    let ptr = alloc_string(_py, value.as_bytes());
    assert!(!ptr.is_null(), "alloc string failed for {value:?}");
    MoltObject::from_ptr(ptr).bits()
}

fn assert_string_attr(
    _py: &PyToken<'_>,
    target_bits: u64,
    slot: &AtomicU64,
    name: &'static [u8],
    expected: &str,
) {
    let attr_name = intern_static_name(_py, slot, name);
    let attr_bits = getattr_optional_bits(_py, target_bits, attr_name)
        .expect("attribute lookup should not raise")
        .unwrap_or_else(|| panic!("missing attribute {}", String::from_utf8_lossy(name)));
    assert_eq!(
        string_obj_to_owned(obj_from_bits(attr_bits)).as_deref(),
        Some(expected),
        "attribute {}",
        String::from_utf8_lossy(name)
    );
    if !obj_from_bits(attr_bits).is_none() {
        dec_ref_bits(_py, attr_bits);
    }
}

#[test]
fn importlib_module_from_spec_impl_materializes_module_without_util_callback() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();

    crate::with_gil_entry_nopanic!(_py, {
        let module_name_bits = alloc_test_string_bits(_py, "pkg.helper");
        // Use the real ModuleSpec producer, including its required
        // has_location=False fact and derived parent, rather than an incomplete
        // module-shaped stand-in for the factory protocol.
        let spec_bits = crate::builtins::types::alloc_module_spec(
            _py,
            module_name_bits,
            MoltObject::none().bits(),
            MoltObject::none().bits(),
            MoltObject::from_bool(false).bits(),
        )
        .expect("actual ModuleSpec fixture");

        let module_bits = importlib_ffi::importlib_module_from_spec_impl(_py, spec_bits);
        assert!(
            !exception_pending(_py),
            "actual ModuleSpec must satisfy the factory protocol: {:?}",
            pending_exception_kind_and_message(_py)
        );
        assert!(!obj_from_bits(module_bits).is_none());

        let spec_attr_name =
            intern_static_name(_py, runtime_static_name_slot(_py, b"__spec__"), b"__spec__");
        let spec_attr_bits = getattr_optional_bits(_py, module_bits, spec_attr_name)
            .expect("__spec__ lookup should not raise")
            .expect("__spec__ must be installed");
        assert_eq!(spec_attr_bits, spec_bits);
        dec_ref_bits(_py, spec_attr_bits);

        assert_string_attr(
            _py,
            module_bits,
            runtime_static_name_slot(_py, b"__name__"),
            b"__name__",
            "pkg.helper",
        );
        assert_string_attr(
            _py,
            module_bits,
            runtime_static_name_slot(_py, b"__package__"),
            b"__package__",
            "pkg",
        );

        dec_ref_bits(_py, module_bits);
        dec_ref_bits(_py, module_name_bits);
        dec_ref_bits(_py, spec_bits);
    });
}

fn call_extension_loader_boundary(_py: &PyToken<'_>, module_name: &str, path: &str) -> u64 {
    let module_bits = alloc_test_string_bits(_py, module_name);
    let path_bits = alloc_test_string_bits(_py, path);
    let out = molt_importlib_extension_loader_payload(
        module_bits,
        path_bits,
        MoltObject::from_bool(false).bits(),
    );
    dec_ref_bits(_py, module_bits);
    dec_ref_bits(_py, path_bits);
    out
}

fn call_extension_exec_boundary(_py: &PyToken<'_>, module_name: &str, path: &str) -> u64 {
    let module_name_bits = alloc_test_string_bits(_py, module_name);
    let path_bits = alloc_test_string_bits(_py, path);
    let module_bits = crate::molt_module_new(module_name_bits);
    let out = match importlib_ffi::bootstrap_loader::importlib_exec_extension_impl(
        _py,
        module_bits,
        module_name_bits,
        module_name,
        path,
    ) {
        Ok(()) => MoltObject::none().bits(),
        Err(bits) => bits,
    };
    dec_ref_bits(_py, module_bits);
    dec_ref_bits(_py, module_name_bits);
    dec_ref_bits(_py, path_bits);
    out
}

fn extension_spec_bits_for_tests(_py: &PyToken<'_>, module_name: &str, origin: &str) -> u64 {
    let spec_bits =
        unsafe { call_callable0(_py, crate::builtins::types::simplenamespace_class(_py)) };
    assert!(
        !obj_from_bits(spec_bits).is_none(),
        "failed to create synthetic spec object"
    );
    assert!(
        !exception_pending(_py),
        "failed to instantiate synthetic spec object: {:?}",
        pending_exception_kind_and_message(_py)
    );
    let module_name_bits = alloc_test_string_bits(_py, module_name);
    let rc = unsafe {
        crate::c_api::molt_object_setattr_bytes(
            spec_bits,
            b"name".as_ptr(),
            b"name".len() as u64,
            module_name_bits,
        )
    };
    assert_eq!(
        rc,
        0,
        "set synthetic spec name failed: {:?}",
        pending_exception_kind_and_message(_py)
    );
    dec_ref_bits(_py, module_name_bits);
    let origin_bits = alloc_test_string_bits(_py, origin);
    let rc = unsafe {
        crate::c_api::molt_object_setattr_bytes(
            spec_bits,
            b"origin".as_ptr(),
            b"origin".len() as u64,
            origin_bits,
        )
    };
    assert_eq!(
        rc,
        0,
        "set synthetic spec origin failed: {:?}",
        pending_exception_kind_and_message(_py)
    );
    dec_ref_bits(_py, origin_bits);
    spec_bits
}

#[test]
fn pending_exception_diagnostic_preserves_message_identity_and_owner() {
    let _guard = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        assert!(pending_exception_kind_and_message(py).is_none());
        let _ = raise_exception::<u64>(py, "ImportError", "extension metadata: exact diagnostic");
        let original =
            crate::builtins::exceptions::ExceptionValue::adopt(py, molt_exception_last_pending());
        let ptr = obj_from_bits(original.bits())
            .as_ptr()
            .expect("raised error");
        let bridge = &molt_cpython_abi::bridge::GLOBAL_BRIDGE;
        // The native __str__ slot can lazily acquire a canonical C-view hold.
        // Reuse the collector's owner accounting, then require actual view
        // retirement below, as the unraisable exception tests do.
        let refs = || {
            (unsafe { (*crate::header_from_obj_ptr(ptr)).ref_count_snapshot() }) as isize
                + bridge.gc_ref_adjustment(original.bits())
        };
        let baseline = refs();
        for _ in 0..2 {
            assert_eq!(
                pending_exception_kind_and_message(py),
                Some((
                    "ImportError".to_owned(),
                    "extension metadata: exact diagnostic".to_owned(),
                ))
            );
            assert!(exception_pending(py));
            let restored = crate::builtins::exceptions::ExceptionValue::adopt(
                py,
                molt_exception_last_pending(),
            );
            assert_eq!(
                restored.bits(),
                original.bits(),
                "restore the exact raised owner"
            );
            drop(restored);
            assert_eq!(
                refs(),
                baseline,
                "diagnostic observation must not retain an owner"
            );
            assert!(
                !bridge.has_direct_c_refs(original.bits()),
                "diagnostic C owners must retire"
            );
        }
        let view = unsafe { bridge.handle_to_borrowed_pyobj(original.bits()) };
        assert!(!view.is_null());
        clear_exception(py);
        assert!(pending_exception_kind_and_message(py).is_none());
        assert_eq!(
            refs(),
            baseline - 1,
            "clearing releases the exact raised owner"
        );
        drop(original);
        assert!(
            bridge.managed_handle_for_pyobj(view).is_none(),
            "the final diagnostic owner must retire its canonical C view"
        );
    });
}

fn assert_pending_exception_contains(_py: &PyToken<'_>, expected_kind: &str, fragments: &[&str]) {
    let (kind, message) =
        pending_exception_kind_and_message(_py).expect("expected pending exception");
    assert_eq!(
        kind, expected_kind,
        "unexpected exception kind: {kind} ({message})"
    );
    for fragment in fragments {
        assert!(
            message.contains(fragment),
            "expected fragment {fragment:?} in exception message {message:?}"
        );
    }
    assert!(
        clear_pending_if_kind(_py, &[expected_kind]),
        "failed to clear pending {expected_kind} exception"
    );
    assert!(!exception_pending(_py));
}

fn write_valid_extension_manifest(
    manifest_path: &std::path::Path,
    module_name: &str,
    extension_entry: &str,
    extension_sha256: &str,
) {
    let abi_major = crate::c_api::MOLT_C_API_VERSION;
    let manifest = serde_json::json!({
        "module": module_name,
        "molt_c_api_version": format!("{abi_major}.0.0"),
        "abi_tag": format!("molt_abi{abi_major}"),
        "target_triple": "test-target",
        "platform_tag": "test-platform",
        "extension": extension_entry,
        "extension_sha256": extension_sha256,
        "capabilities": ["fs.read"],
    });
    let bytes = serde_json::to_vec(&manifest).expect("encode extension manifest");
    std::fs::write(manifest_path, bytes).expect("write extension manifest");
}

#[test]
fn sys_bootstrap_state_ignores_pythonpath_and_includes_module_roots_and_pwd() {
    let sep = if sys_platform_str().starts_with("win") {
        ';'
    } else {
        ':'
    };
    let py_path = format!("alpha{sep}beta");
    let module_roots = format!("gamma{sep}beta{sep}delta");
    with_env_state(
        &[
            ("PYTHONPATH", &py_path),
            ("MOLT_MODULE_ROOTS", &module_roots),
            ("MOLT_CAPABILITY_TIER", "full"),
            ("PWD", "/tmp/molt_pwd"),
        ],
        || {
            let state = crate::with_gil_entry_nopanic!(py, {
                sys_bootstrap_state_from_module_file(py, Some(bootstrap_module_file()))
            });
            let path_sep = if sys_platform_str().starts_with("win") {
                '\\'
            } else {
                '/'
            };
            let expected_beta = bootstrap_resolve_path_entry("beta", "/tmp/molt_pwd", path_sep);
            let expected_gamma = bootstrap_resolve_path_entry("gamma", "/tmp/molt_pwd", path_sep);
            let expected_delta = bootstrap_resolve_path_entry("delta", "/tmp/molt_pwd", path_sep);
            assert!(state.pythonpath_entries.is_empty());
            assert_eq!(
                state.module_roots_entries,
                vec![
                    expected_gamma.clone(),
                    expected_beta.clone(),
                    expected_delta.clone()
                ]
            );
            assert_eq!(state.stdlib_root, Some(expected_stdlib_root()));
            assert_eq!(state.pwd, "/tmp/molt_pwd");
            assert!(state.include_cwd);
            assert_eq!(
                state.path,
                vec![
                    "".to_string(),
                    expected_stdlib_root(),
                    expected_gamma,
                    expected_beta,
                    expected_delta,
                ]
            );
        },
    );
}

#[test]
fn sys_bootstrap_state_omits_cwd_when_dev_untrusted() {
    let sep = if sys_platform_str().starts_with("win") {
        ';'
    } else {
        ':'
    };
    let py_path = format!("alpha{sep}beta");
    with_env_state(
        &[
            ("PYTHONPATH", &py_path),
            ("MOLT_MODULE_ROOTS", ""),
            ("MOLT_CAPABILITY_TIER", "none"),
            ("PWD", "/tmp/molt_pwd"),
        ],
        || {
            let state = crate::with_gil_entry_nopanic!(py, {
                sys_bootstrap_state_from_module_file(py, Some(bootstrap_module_file()))
            });
            assert!(!state.include_cwd);
            assert!(!state.path.iter().any(|entry| entry.is_empty()));
            assert!(!state.path.iter().any(|entry| entry == "/tmp/molt_pwd"));
        },
    );
}

#[test]
fn sys_bootstrap_state_normalizes_stdlib_root_for_stdlib_submodule() {
    with_env_state(
        &[
            ("PYTHONPATH", ""),
            ("MOLT_MODULE_ROOTS", ""),
            ("MOLT_CAPABILITY_TIER", "full"),
            ("PWD", "/tmp/molt_pwd"),
        ],
        || {
            let state = crate::with_gil_entry_nopanic!(py, {
                sys_bootstrap_state_from_module_file(py, Some(bootstrap_stdlib_submodule_file()))
            });
            assert_eq!(state.stdlib_root, Some(expected_stdlib_root()));
            assert!(
                state
                    .path
                    .iter()
                    .any(|entry| entry == &expected_stdlib_root())
            );
        },
    );
}

#[test]
fn sys_bootstrap_state_falls_back_to_current_dir_when_pwd_missing() {
    with_env_state(
        &[
            ("PYTHONPATH", ""),
            ("MOLT_MODULE_ROOTS", ""),
            ("MOLT_CAPABILITY_TIER", "full"),
            ("PWD", ""),
        ],
        || {
            let state = crate::with_gil_entry_nopanic!(py, {
                sys_bootstrap_state_from_module_file(py, Some(bootstrap_module_file()))
            });
            assert!(state.include_cwd);
            assert_eq!(state.pwd, resolve_bootstrap_pwd(""));
            assert!(state.path.iter().any(|entry| entry.is_empty()));
        },
    );
}

#[test]
fn sys_bootstrap_state_ignores_virtual_env_site_packages_when_present() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!(
        "molt_virtual_env_bootstrap_{}_{}",
        std::process::id(),
        stamp
    ));
    let venv_root = tmp.join("venv");
    let site_packages = if sys_platform_str().starts_with("win") {
        venv_root.join("Lib").join("site-packages")
    } else {
        venv_root
            .join("lib")
            .join("python3.12")
            .join("site-packages")
    };
    std::fs::create_dir_all(&site_packages).expect("create virtualenv site-packages");
    let venv_root_text = venv_root.to_string_lossy().into_owned();
    let site_packages_text = site_packages.to_string_lossy().into_owned();
    with_env_state(
        &[
            ("PYTHONPATH", ""),
            ("MOLT_MODULE_ROOTS", ""),
            ("VIRTUAL_ENV", &venv_root_text),
            ("MOLT_CAPABILITY_TIER", "full"),
            ("PWD", "/tmp/molt_pwd"),
        ],
        || {
            let state = crate::with_gil_entry_nopanic!(py, {
                sys_bootstrap_state_from_module_file(py, Some(bootstrap_module_file()))
            });
            assert!(state.venv_site_packages_entries.is_empty());
            assert!(!state.path.iter().any(|entry| entry == &site_packages_text));
        },
    );
    std::fs::remove_dir_all(&tmp).expect("cleanup virtualenv temp dirs");
}

#[test]
fn bootstrap_resolve_abspath_uses_bootstrap_pwd_for_relative_paths() {
    let sep = bootstrap_path_sep();
    let pwd = platform_test_path(&["bootstrap_pwd"]);
    let rel_path = path_join_text(path_join_text("pkg".to_string(), "..", sep), "mod.py", sep);
    let expected = path_join_text(pwd.clone(), "mod.py", sep);
    with_env_state(
        &[
            ("PYTHONPATH", ""),
            ("MOLT_MODULE_ROOTS", ""),
            ("MOLT_CAPABILITY_TIER", "full"),
            ("PWD", &pwd),
        ],
        || {
            let resolved = crate::with_gil_entry_nopanic!(py, {
                bootstrap_resolve_abspath(py, &rel_path, Some(bootstrap_module_file()))
            });
            assert_eq!(resolved, expected);
        },
    );
}

#[test]
fn importlib_source_loader_resolution_marks_packages() {
    let package_path = platform_test_path(&["demo", "pkg", "__init__.py"]);
    let package = source_loader_resolution("demo.pkg", &package_path, false);
    assert!(package.is_package);
    assert_eq!(package.module_package, "demo.pkg");
    assert_eq!(
        package.package_root,
        Some(path_dirname_text(&package_path, bootstrap_path_sep()))
    );

    let module_path = platform_test_path(&["demo", "pkg", "mod.py"]);
    let module = source_loader_resolution("demo.pkg.mod", &module_path, false);
    assert!(!module.is_package);
    assert_eq!(module.module_package, "demo.pkg");
    assert_eq!(module.package_root, None);
}

#[test]
fn importlib_cache_from_source_matches_cpython_layout() {
    let sep = bootstrap_path_sep();
    let module_path = platform_test_path(&["pkg", "mod.py"]);
    let cache_dir = path_join_text(path_dirname_text(&module_path, sep), "__pycache__", sep);
    assert_eq!(
        importlib_cache_from_source(&module_path, "implementation-312", ".pyc"),
        path_join_text(cache_dir.clone(), "mod.implementation-312.pyc", sep)
    );
    let extensionless = platform_test_path(&["pkg", "mod"]);
    assert_eq!(
        importlib_cache_from_source(&extensionless, "implementation-312", ".pyc"),
        path_join_text(cache_dir.clone(), "modimplementation-312.pyc", sep)
    );
    for (filename, expected) in [
        ("mod.custom", "mod.implementation-312.pyc"),
        (".hidden", "hidden.implementation-312.pyc"),
        ("mod.one.py", "mod.one.implementation-312.pyc"),
    ] {
        let path = platform_test_path(&["pkg", filename]);
        assert_eq!(
            importlib_cache_from_source(&path, "implementation-312", ".pyc"),
            path_join_text(cache_dir.clone(), expected, sep)
        );
    }
}

#[test]
fn importlib_find_in_path_resolves_package_and_module() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!(
        "molt_importlib_find_spec_{}_{}",
        std::process::id(),
        stamp
    ));
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let pkg_dir = tmp.join("pkgdemo");
    std::fs::create_dir_all(&pkg_dir).expect("create package dir");
    std::fs::write(pkg_dir.join("__init__.py"), "value = 1\n").expect("write __init__.py");
    std::fs::write(tmp.join("moddemo.py"), "value = 2\n").expect("write module file");

    let search_paths = vec![tmp.to_string_lossy().into_owned()];
    let pkg = importlib_find_in_path("pkgdemo", &search_paths, false).expect("package spec");
    assert!(pkg.is_package);
    let pkg_origin = pkg.origin.clone().expect("package origin");
    assert!(pkg_origin.ends_with("__init__.py"));
    assert_eq!(
        pkg.submodule_search_locations,
        Some(vec![pkg_dir.to_string_lossy().into_owned()])
    );
    assert_eq!(pkg.cached, None);
    assert!(pkg.has_location);
    assert_eq!(pkg.loader_kind, "source");

    let module = importlib_find_in_path("moddemo", &search_paths, false).expect("module spec");
    assert!(!module.is_package);
    let module_origin = module.origin.clone().expect("module origin");
    assert!(module_origin.ends_with("moddemo.py"));
    assert_eq!(module.submodule_search_locations, None);
    assert_eq!(module.cached, None);
    assert!(module.has_location);
    assert_eq!(module.loader_kind, "source");

    std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
}

#[test]
fn importlib_find_in_path_resolves_namespace_package() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!(
        "molt_importlib_namespace_spec_{}_{}",
        std::process::id(),
        stamp
    ));
    let left_root = tmp.join("left");
    let right_root = tmp.join("right");
    let left_ns = left_root.join("nspkg");
    let right_ns = right_root.join("nspkg");
    std::fs::create_dir_all(&left_ns).expect("create left namespace path");
    std::fs::create_dir_all(&right_ns).expect("create right namespace path");
    std::fs::write(right_ns.join("mod.py"), "value = 1\n").expect("write module file");

    let search_paths = vec![
        left_root.to_string_lossy().into_owned(),
        right_root.to_string_lossy().into_owned(),
    ];
    let namespace = importlib_find_in_path("nspkg", &search_paths, false).expect("namespace spec");
    assert!(namespace.is_package);
    assert_eq!(namespace.origin, None);
    assert_eq!(namespace.cached, None);
    assert!(!namespace.has_location);
    assert_eq!(namespace.loader_kind, "namespace");
    assert_eq!(
        namespace.submodule_search_locations,
        Some(vec![
            left_ns.to_string_lossy().into_owned(),
            right_ns.to_string_lossy().into_owned(),
        ])
    );

    let module = importlib_find_in_path("nspkg.mod", &search_paths, false).expect("module spec");
    let module_origin = module.origin.clone().expect("module origin");
    assert!(module_origin.ends_with("mod.py"));
    assert!(!module.is_package);
    assert_eq!(module.loader_kind, "source");

    std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
}

#[test]
fn importlib_find_in_path_resolves_extension_module() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!(
        "molt_importlib_extension_spec_{}_{}",
        std::process::id(),
        stamp
    ));
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let ext_path = tmp.join("extdemo.so");
    std::fs::write(&ext_path, b"").expect("write extension placeholder");

    let search_paths = vec![tmp.to_string_lossy().into_owned()];
    let module = importlib_find_in_path("extdemo", &search_paths, false).expect("module spec");
    let module_origin = module.origin.clone().expect("module origin");
    assert!(module_origin.ends_with("extdemo.so"));
    assert!(!module.is_package);
    assert_eq!(module.submodule_search_locations, None);
    assert_eq!(module.cached, None);
    assert!(module.has_location);
    assert_eq!(module.loader_kind, "extension");

    std::fs::write(tmp.join("notext.fake.so"), b"").expect("write invalid extension name");
    let missing = importlib_find_in_path("notext", &search_paths, false);
    assert!(missing.is_none());

    std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
}

#[test]
fn extension_path_matches_manifest_entry_variants() {
    assert!(importlib_extension_path_matches_manifest(
        "/tmp/site/demo/native.so",
        "demo/native.so"
    ));
    assert!(importlib_extension_path_matches_manifest(
        "/tmp/site/demo/native.so",
        "native.so"
    ));
    assert!(importlib_extension_path_matches_manifest(
        "C:\\site\\demo\\native.pyd",
        "demo/native.pyd"
    ));
    assert!(!importlib_extension_path_matches_manifest(
        "/tmp/site/demo/other.so",
        "demo/native.so"
    ));
}

#[test]
fn find_extension_manifest_sidecar_walks_parent_dirs() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!(
        "molt_extension_manifest_sidecar_{}_{}",
        std::process::id(),
        stamp
    ));
    let pkg_dir = tmp.join("pkg").join("nested");
    std::fs::create_dir_all(&pkg_dir).expect("create extension dir");
    let extension_path = pkg_dir.join("native.so");
    std::fs::write(&extension_path, b"binary").expect("write extension placeholder");
    let manifest_path = tmp.join("extension_manifest.json");
    std::fs::write(&manifest_path, b"{}\n").expect("write manifest");

    let found = importlib_find_extension_manifest_sidecar(&extension_path.to_string_lossy())
        .expect("resolve sidecar");
    assert_eq!(found, Some(manifest_path.to_string_lossy().into_owned()));

    std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
}

#[test]
fn extension_cache_fingerprint_changes_when_binary_changes() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!(
        "molt_extension_cache_fingerprint_{}_{}",
        std::process::id(),
        stamp
    ));
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let extension_path = tmp.join("native.so");
    std::fs::write(&extension_path, b"abc").expect("write extension");
    let first = importlib_cache_fingerprint_for_path(&extension_path.to_string_lossy())
        .expect("first fingerprint");
    std::fs::write(&extension_path, b"abcdef012345").expect("rewrite extension");
    let second = importlib_cache_fingerprint_for_path(&extension_path.to_string_lossy())
        .expect("second fingerprint");
    assert_ne!(first, second);
    std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
}

#[test]
fn extension_manifest_cache_fingerprint_changes_when_sidecar_changes() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!(
        "molt_manifest_cache_fingerprint_{}_{}",
        std::process::id(),
        stamp
    ));
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let manifest_path = tmp.join("extension_manifest.json");
    std::fs::write(&manifest_path, b"{\"module\":\"demo\"}\n").expect("write manifest");
    let loaded = LoadedExtensionManifest {
        source: manifest_path.to_string_lossy().into_owned(),
        manifest: JsonValue::Null,
        wheel_path: None,
    };
    let first = importlib_manifest_cache_fingerprint(&loaded).expect("first fingerprint");
    std::fs::write(
        &manifest_path,
        b"{\"module\":\"demo\",\"capabilities\":[\"fs.read\"]}\n",
    )
    .expect("rewrite manifest");
    let second = importlib_manifest_cache_fingerprint(&loaded).expect("second fingerprint");
    assert_ne!(first, second);
    std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
}

#[test]
fn extension_spec_boundary_rejects_missing_manifest_sidecar() {
    crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
        clear_extension_metadata_validation_cache();
        let tmp = extension_boundary_temp_dir("molt_extension_spec_missing_manifest");
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let module_name = format!("ext_spec_missing_{}", std::process::id());
        let filename = extension_boundary_module_filename(&module_name);
        let extension_path = tmp.join(&filename);
        std::fs::write(&extension_path, b"spec-boundary-extension")
            .expect("write extension placeholder");
        let search_paths = vec![tmp.to_string_lossy().into_owned()];

        crate::with_gil_entry_nopanic!(_py, {
            let out = importlib_resolve_spec_in_paths(_py, &module_name, &search_paths, false);
            assert!(
                out.is_err(),
                "expected spec boundary failure for missing manifest"
            );
            assert_pending_exception_contains(
                _py,
                "ImportError",
                &[
                    "extension metadata missing",
                    "extension_manifest.json not found near extension path",
                ],
            );
        });

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    });
}

#[test]
fn extension_spec_boundary_rejects_invalid_manifest_payload() {
    crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
        clear_extension_metadata_validation_cache();
        let tmp = extension_boundary_temp_dir("molt_extension_spec_invalid_manifest");
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let module_name = format!("ext_spec_invalid_{}", std::process::id());
        let filename = extension_boundary_module_filename(&module_name);
        let extension_path = tmp.join(&filename);
        std::fs::write(&extension_path, b"spec-boundary-extension")
            .expect("write extension placeholder");
        std::fs::write(tmp.join("extension_manifest.json"), b"{not-json}\n")
            .expect("write invalid metadata manifest");
        let search_paths = vec![tmp.to_string_lossy().into_owned()];

        crate::with_gil_entry_nopanic!(_py, {
            let out = importlib_resolve_spec_in_paths(_py, &module_name, &search_paths, false);
            assert!(
                out.is_err(),
                "expected spec boundary failure for invalid manifest"
            );
            assert_pending_exception_contains(
                _py,
                "ImportError",
                &["invalid extension metadata in", "extension_manifest.json"],
            );
        });

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    });
}

#[test]
fn extension_spec_boundary_accepts_valid_manifest() {
    crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
        clear_extension_metadata_validation_cache();
        let tmp = extension_boundary_temp_dir("molt_extension_spec_valid_manifest");
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let module_name = format!("ext_spec_valid_{}", std::process::id());
        let filename = extension_boundary_module_filename(&module_name);
        let extension_path = tmp.join(&filename);
        std::fs::write(&extension_path, b"spec-boundary-extension")
            .expect("write extension placeholder");
        let extension_path_text = extension_path.to_string_lossy().into_owned();
        let extension_sha256 =
            importlib_sha256_file(&extension_path_text).expect("hash extension placeholder");
        write_valid_extension_manifest(
            &tmp.join("extension_manifest.json"),
            &module_name,
            &filename,
            &extension_sha256,
        );
        let search_paths = vec![tmp.to_string_lossy().into_owned()];

        crate::with_gil_entry_nopanic!(_py, {
            let payload = importlib_resolve_spec_in_paths(_py, &module_name, &search_paths, false)
                .expect("spec boundary should pass")
                .expect("extension spec should resolve");
            assert_eq!(payload.loader_kind, "extension");
            assert_eq!(
                payload.origin.as_deref(),
                Some(extension_path_text.as_str())
            );
            assert!(
                !exception_pending(_py),
                "unexpected pending exception after valid spec boundary check: {:?}",
                pending_exception_kind_and_message(_py)
            );
        });

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    });
}

#[test]
fn extension_spec_boundary_rejects_manifest_module_mismatch() {
    crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
        clear_extension_metadata_validation_cache();
        let tmp = extension_boundary_temp_dir("molt_extension_spec_module_mismatch");
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let module_name = format!("ext_spec_requested_{}", std::process::id());
        let manifest_module_name = format!("ext_spec_manifest_{}", std::process::id());
        let filename = extension_boundary_module_filename(&module_name);
        let extension_path = tmp.join(&filename);
        std::fs::write(&extension_path, b"spec-boundary-extension")
            .expect("write extension placeholder");
        let extension_path_text = extension_path.to_string_lossy().into_owned();
        let extension_sha256 =
            importlib_sha256_file(&extension_path_text).expect("hash extension placeholder");
        write_valid_extension_manifest(
            &tmp.join("extension_manifest.json"),
            &manifest_module_name,
            &filename,
            &extension_sha256,
        );
        let search_paths = vec![tmp.to_string_lossy().into_owned()];

        crate::with_gil_entry_nopanic!(_py, {
            let out = importlib_resolve_spec_in_paths(_py, &module_name, &search_paths, false);
            assert!(
                out.is_err(),
                "expected spec boundary failure for manifest module mismatch"
            );
            assert_pending_exception_contains(
                _py,
                "ImportError",
                &["extension metadata module mismatch"],
            );
        });

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    });
}

#[test]
fn extension_spec_boundary_revalidates_cache_after_artifact_mutation() {
    crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
        clear_extension_metadata_validation_cache();
        let tmp = extension_boundary_temp_dir("molt_extension_spec_cache_revalidation");
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let module_name = format!("ext_spec_cache_{}", std::process::id());
        let filename = extension_boundary_module_filename(&module_name);
        let extension_path = tmp.join(&filename);
        std::fs::write(&extension_path, b"spec-boundary-extension-v1")
            .expect("write extension placeholder");
        let extension_path_text = extension_path.to_string_lossy().into_owned();
        let extension_sha256 =
            importlib_sha256_file(&extension_path_text).expect("hash extension placeholder");
        write_valid_extension_manifest(
            &tmp.join("extension_manifest.json"),
            &module_name,
            &filename,
            &extension_sha256,
        );
        let search_paths = vec![tmp.to_string_lossy().into_owned()];

        crate::with_gil_entry_nopanic!(_py, {
            let payload = importlib_resolve_spec_in_paths(_py, &module_name, &search_paths, false)
                .expect("first spec boundary pass should succeed")
                .expect("first extension spec should resolve");
            assert_eq!(payload.loader_kind, "extension");
            assert_eq!(
                payload.origin.as_deref(),
                Some(extension_path_text.as_str())
            );
        });

        std::fs::write(&extension_path, b"spec-boundary-extension-v2-changed")
            .expect("mutate extension artifact");

        crate::with_gil_entry_nopanic!(_py, {
            let out = importlib_resolve_spec_in_paths(_py, &module_name, &search_paths, false);
            assert!(
                out.is_err(),
                "expected spec boundary failure after extension mutation"
            );
            assert_pending_exception_contains(_py, "ImportError", &["extension checksum mismatch"]);
        });

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    });
}

#[test]
fn extension_spec_object_boundary_enforces_missing_and_valid_manifest() {
    crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
        clear_extension_metadata_validation_cache();
        {
            let tmp = extension_boundary_temp_dir("molt_extension_spec_object_missing_manifest");
            std::fs::create_dir_all(&tmp).expect("create temp dir");
            let module_name = format!("ext_spec_object_missing_{}", std::process::id());
            let filename = extension_boundary_module_filename(&module_name);
            let extension_path = tmp.join(&filename);
            std::fs::write(&extension_path, b"spec-object-boundary-extension")
                .expect("write extension placeholder");
            let extension_path_text = extension_path.to_string_lossy().into_owned();

            crate::with_gil_entry_nopanic!(_py, {
                let spec_bits =
                    extension_spec_bits_for_tests(_py, &module_name, &extension_path_text);
                let out =
                    importlib_enforce_extension_spec_object_boundary(_py, &module_name, spec_bits);
                assert!(
                    out.is_err(),
                    "expected extension spec object boundary failure for missing manifest"
                );
                assert_pending_exception_contains(
                    _py,
                    "ImportError",
                    &[
                        "extension metadata missing",
                        "extension_manifest.json not found near extension path",
                    ],
                );
                dec_ref_bits(_py, spec_bits);
            });

            std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
        }

        clear_extension_metadata_validation_cache();

        {
            let tmp = extension_boundary_temp_dir("molt_extension_spec_object_valid_manifest");
            std::fs::create_dir_all(&tmp).expect("create temp dir");
            let module_name = format!("ext_spec_object_valid_{}", std::process::id());
            let filename = extension_boundary_module_filename(&module_name);
            let extension_path = tmp.join(&filename);
            std::fs::write(&extension_path, b"spec-object-boundary-extension")
                .expect("write extension placeholder");
            let extension_path_text = extension_path.to_string_lossy().into_owned();
            let extension_sha256 =
                importlib_sha256_file(&extension_path_text).expect("hash extension placeholder");
            write_valid_extension_manifest(
                &tmp.join("extension_manifest.json"),
                &module_name,
                &filename,
                &extension_sha256,
            );

            crate::with_gil_entry_nopanic!(_py, {
                let spec_bits =
                    extension_spec_bits_for_tests(_py, &module_name, &extension_path_text);
                let out =
                    importlib_enforce_extension_spec_object_boundary(_py, &module_name, spec_bits);
                dec_ref_bits(_py, spec_bits);
                assert!(
                    out.is_ok(),
                    "unexpected extension spec object boundary failure: {:?}",
                    pending_exception_kind_and_message(_py)
                );
                assert!(
                    !exception_pending(_py),
                    "unexpected pending exception after successful spec object boundary check: {:?}",
                    pending_exception_kind_and_message(_py)
                );
            });

            std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
        }
    });
}

#[test]
fn extension_loader_boundary_rejects_missing_manifest_sidecar() {
    crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
        clear_extension_metadata_validation_cache();
        let tmp = extension_boundary_temp_dir("molt_extension_loader_missing_manifest");
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let extension_path = tmp.join(extension_boundary_filename());
        std::fs::write(&extension_path, b"loader-boundary-extension")
            .expect("write extension placeholder");
        let module_name = "demo.extension.loader.missing";
        let extension_path_text = extension_path.to_string_lossy().into_owned();

        crate::with_gil_entry_nopanic!(_py, {
            let _ = call_extension_loader_boundary(_py, module_name, &extension_path_text);
            assert_pending_exception_contains(
                _py,
                "ImportError",
                &[
                    "extension metadata missing",
                    "extension_manifest.json not found near extension path",
                ],
            );
        });

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    });
}

#[test]
fn extension_loader_boundary_rejects_invalid_manifest_payload() {
    crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
        clear_extension_metadata_validation_cache();
        let tmp = extension_boundary_temp_dir("molt_extension_loader_invalid_manifest");
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let extension_path = tmp.join(extension_boundary_filename());
        std::fs::write(&extension_path, b"loader-boundary-extension")
            .expect("write extension placeholder");
        std::fs::write(tmp.join("extension_manifest.json"), b"{not-json}\n")
            .expect("write invalid manifest");
        let module_name = "demo.extension.loader.invalid";
        let extension_path_text = extension_path.to_string_lossy().into_owned();

        crate::with_gil_entry_nopanic!(_py, {
            let _ = call_extension_loader_boundary(_py, module_name, &extension_path_text);
            assert_pending_exception_contains(
                _py,
                "ImportError",
                &["invalid extension metadata in", "extension_manifest.json"],
            );
        });

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    });
}

#[test]
fn extension_exec_boundary_rejects_missing_manifest_sidecar() {
    crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
        clear_extension_metadata_validation_cache();
        let tmp = extension_boundary_temp_dir("molt_extension_exec_missing_manifest");
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let extension_path = tmp.join(extension_boundary_filename());
        std::fs::write(&extension_path, b"exec-boundary-extension")
            .expect("write extension placeholder");
        let module_name = "demo.extension.exec.missing";
        let extension_path_text = extension_path.to_string_lossy().into_owned();

        crate::with_gil_entry_nopanic!(_py, {
            let _ = call_extension_exec_boundary(_py, module_name, &extension_path_text);
            assert_pending_exception_contains(
                _py,
                "ImportError",
                &[
                    "extension metadata missing",
                    "extension_manifest.json not found near extension path",
                ],
            );
        });

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    });
}

#[test]
fn extension_exec_boundary_rejects_invalid_manifest_metadata() {
    crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
        clear_extension_metadata_validation_cache();
        let tmp = extension_boundary_temp_dir("molt_extension_exec_invalid_manifest");
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let extension_path = tmp.join(extension_boundary_filename());
        std::fs::write(&extension_path, b"exec-boundary-extension")
            .expect("write extension placeholder");
        std::fs::write(tmp.join("extension_manifest.json"), b"{}\n")
            .expect("write invalid metadata manifest");
        let module_name = "demo.extension.exec.invalid";
        let extension_path_text = extension_path.to_string_lossy().into_owned();

        crate::with_gil_entry_nopanic!(_py, {
            let _ = call_extension_exec_boundary(_py, module_name, &extension_path_text);
            assert_pending_exception_contains(
                _py,
                "ImportError",
                &["missing or invalid field", "\"module\""],
            );
        });

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    });
}

#[test]
fn extension_loader_boundary_rejects_manifest_module_mismatch() {
    crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
        clear_extension_metadata_validation_cache();
        let tmp = extension_boundary_temp_dir("molt_extension_loader_module_mismatch");
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let extension_path = tmp.join(extension_boundary_filename());
        std::fs::write(&extension_path, b"loader-boundary-extension")
            .expect("write extension placeholder");
        let module_name = "demo.extension.loader.requested";
        let manifest_module_name = "demo.extension.loader.manifest";
        let extension_path_text = extension_path.to_string_lossy().into_owned();
        let extension_sha256 =
            importlib_sha256_file(&extension_path_text).expect("hash extension placeholder");
        write_valid_extension_manifest(
            &tmp.join("extension_manifest.json"),
            manifest_module_name,
            extension_boundary_filename(),
            &extension_sha256,
        );

        crate::with_gil_entry_nopanic!(_py, {
            let _ = call_extension_loader_boundary(_py, module_name, &extension_path_text);
            assert_pending_exception_contains(
                _py,
                "ImportError",
                &["extension metadata module mismatch"],
            );
        });

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    });
}

#[test]
fn extension_exec_boundary_rejects_manifest_module_mismatch() {
    crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
        clear_extension_metadata_validation_cache();
        let tmp = extension_boundary_temp_dir("molt_extension_exec_module_mismatch");
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let extension_path = tmp.join(extension_boundary_filename());
        std::fs::write(&extension_path, b"exec-boundary-extension")
            .expect("write extension placeholder");
        let module_name = "demo.extension.exec.requested";
        let manifest_module_name = "demo.extension.exec.manifest";
        let extension_path_text = extension_path.to_string_lossy().into_owned();
        let extension_sha256 =
            importlib_sha256_file(&extension_path_text).expect("hash extension placeholder");
        write_valid_extension_manifest(
            &tmp.join("extension_manifest.json"),
            manifest_module_name,
            extension_boundary_filename(),
            &extension_sha256,
        );

        crate::with_gil_entry_nopanic!(_py, {
            let _ = call_extension_exec_boundary(_py, module_name, &extension_path_text);
            assert_pending_exception_contains(
                _py,
                "ImportError",
                &["extension metadata module mismatch"],
            );
        });

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    });
}

#[test]
fn extension_loader_boundary_revalidates_cache_after_artifact_mutation() {
    crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
        clear_extension_metadata_validation_cache();
        let tmp = extension_boundary_temp_dir("molt_extension_loader_cache_revalidation");
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let extension_path = tmp.join(extension_boundary_filename());
        let initial_extension = b"extension-v1";
        std::fs::write(&extension_path, initial_extension).expect("write extension placeholder");
        let module_name = "demo.extension.loader.cache";
        let extension_path_text = extension_path.to_string_lossy().into_owned();
        let extension_sha256 =
            importlib_sha256_file(&extension_path_text).expect("hash extension placeholder");
        write_valid_extension_manifest(
            &tmp.join("extension_manifest.json"),
            module_name,
            extension_boundary_filename(),
            &extension_sha256,
        );

        crate::with_gil_entry_nopanic!(_py, {
            let payload_bits =
                call_extension_loader_boundary(_py, module_name, &extension_path_text);
            assert!(
                !exception_pending(_py),
                "unexpected boundary exception on first pass: {:?}",
                pending_exception_kind_and_message(_py)
            );
            assert!(
                !obj_from_bits(payload_bits).is_none(),
                "expected loader payload on first pass"
            );
            dec_ref_bits(_py, payload_bits);
        });

        std::fs::write(&extension_path, b"extension-v2-with-different-size")
            .expect("mutate extension artifact");

        crate::with_gil_entry_nopanic!(_py, {
            let _ = call_extension_loader_boundary(_py, module_name, &extension_path_text);
            assert_pending_exception_contains(_py, "ImportError", &["extension checksum mismatch"]);
        });

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    });
}

#[test]
fn extension_loader_boundary_records_cache_hits_and_misses() {
    crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
        clear_extension_metadata_validation_cache();
        let tmp = extension_boundary_temp_dir("molt_extension_loader_cache_hit_miss");
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let extension_path = tmp.join(extension_boundary_filename());
        let module_name = "demo.extension.loader.cache_stats";
        std::fs::write(&extension_path, b"loader-boundary-extension-cache")
            .expect("write extension placeholder");
        let extension_path_text = extension_path.to_string_lossy().into_owned();
        let extension_sha256 =
            importlib_sha256_file(&extension_path_text).expect("hash extension placeholder");
        write_valid_extension_manifest(
            &tmp.join("extension_manifest.json"),
            module_name,
            extension_boundary_filename(),
            &extension_sha256,
        );

        crate::with_gil_entry_nopanic!(_py, {
            let payload_bits =
                call_extension_loader_boundary(_py, module_name, &extension_path_text);
            assert!(
                !exception_pending(_py),
                "unexpected boundary exception on cold validation: {:?}",
                pending_exception_kind_and_message(_py)
            );
            assert!(
                !obj_from_bits(payload_bits).is_none(),
                "expected loader payload on cold validation"
            );
            dec_ref_bits(_py, payload_bits);

            let payload_bits =
                call_extension_loader_boundary(_py, module_name, &extension_path_text);
            assert!(
                !exception_pending(_py),
                "unexpected boundary exception on warm validation: {:?}",
                pending_exception_kind_and_message(_py)
            );
            assert!(
                !obj_from_bits(payload_bits).is_none(),
                "expected loader payload on warm validation"
            );
            dec_ref_bits(_py, payload_bits);
        });

        let (hits, misses) = extension_metadata_cache_stats();
        assert!(
            misses >= 1,
            "expected at least one cold miss, observed hits={hits}, misses={misses}"
        );
        assert!(
            hits >= 1,
            "expected at least one warm hit, observed hits={hits}, misses={misses}"
        );

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    });
}

#[test]
fn importlib_sha256_file_matches_direct_byte_hash() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!(
        "molt_importlib_sha_file_{}_{}",
        std::process::id(),
        stamp
    ));
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let path = tmp.join("payload.bin");
    std::fs::write(&path, b"streamed-hash-payload").expect("write payload");
    let path_text = path.to_string_lossy().into_owned();

    let digest = importlib_sha256_file(&path_text).expect("hash payload");

    assert_eq!(digest, importlib_sha256_hex(b"streamed-hash-payload"));
    std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
}

#[cfg(feature = "stdlib_archive")]
#[test]
#[cfg_attr(miri, ignore)]
fn importlib_sha256_path_supports_zip_archive_members() {
    let _test = crate::test_support::RuntimeTestTransaction::new();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!(
        "molt_importlib_sha_zip_{}_{}",
        std::process::id(),
        stamp
    ));
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let archive = tmp.join("mods.whl");
    let file = std::fs::File::create(&archive).expect("create archive");
    let mut writer = zip::ZipWriter::new(file);
    let options: zip::write::SimpleFileOptions = zip::write::FileOptions::default();
    writer
        .start_file("demo/native.so", options)
        .expect("start archive entry");
    writer
        .write_all(b"zip-extension-bytes")
        .expect("write archive entry");
    writer.finish().expect("finish archive");

    let archive_member_path = format!("{}/demo/native.so", archive.to_string_lossy());
    crate::with_gil_entry_nopanic!(_py, {
        let digest = importlib_sha256_path(_py, &archive_member_path).expect("hash archive member");
        assert_eq!(digest, importlib_sha256_hex(b"zip-extension-bytes"));
    });

    std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
}

#[test]
fn importlib_find_in_path_package_context_resolves_submodule() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!(
        "molt_importlib_package_context_{}_{}",
        std::process::id(),
        stamp
    ));
    let pkg_root = tmp.join("pkg");
    std::fs::create_dir_all(&pkg_root).expect("create package root");
    std::fs::write(pkg_root.join("mod.py"), "value = 3\n").expect("write module file");

    let search_paths = vec![pkg_root.to_string_lossy().into_owned()];
    let module = importlib_find_in_path("pkg.mod", &search_paths, true).expect("module spec");
    let module_origin = module.origin.clone().expect("module origin");
    assert!(module_origin.ends_with("mod.py"));
    assert!(!module.is_package);
    assert_eq!(module.loader_kind, "source");

    std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
}

#[test]
fn importlib_spec_resolution_preserves_explicit_paths_over_bootstrap_roots() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!(
        "molt_importlib_package_context_module_roots_{}_{}",
        std::process::id(),
        stamp
    ));
    let stale_pkg = tmp.join("stale").join("nativepkg");
    let staged_pkg = tmp.join("staged").join("nativepkg");
    std::fs::create_dir_all(&stale_pkg).expect("create stale package root");
    std::fs::create_dir_all(&staged_pkg).expect("create staged package root");
    let stale_ext = stale_pkg.join("_native.so");
    let staged_ext = staged_pkg.join("_native.so");
    std::fs::write(&stale_ext, b"stale-extension").expect("write stale extension");
    std::fs::write(&staged_ext, b"staged-extension").expect("write staged extension");
    write_valid_extension_manifest(
        &stale_pkg.join("extension_manifest.json"),
        "nativepkg._native",
        "_native.so",
        &importlib_sha256_hex(b"stale-extension"),
    );
    write_valid_extension_manifest(
        &staged_pkg.join("extension_manifest.json"),
        "nativepkg._native",
        "_native.so",
        &importlib_sha256_hex(b"staged-extension"),
    );
    let staged_root = tmp.join("staged");
    let module_roots = staged_root.to_string_lossy().into_owned();
    let pwd = tmp.to_string_lossy().into_owned();
    let staged_ext_text = staged_ext.to_string_lossy().into_owned();
    let stale_ext_text = stale_ext.to_string_lossy().into_owned();
    let stale_search_paths = vec![stale_pkg.to_string_lossy().into_owned()];

    // Extension capabilities are admitted at cold bootstrap; the projected
    // environment below independently controls import roots and cwd policy.
    crate::test_support::RuntimeTestTransaction::with_cold_runtime_lifecycle(|| {
        with_env_projection(
            &[
                ("PYTHONPATH", ""),
                ("MOLT_MODULE_ROOTS", &module_roots),
                ("MOLT_CAPABILITY_TIER", "full"),
                ("PWD", &pwd),
            ],
            || {
                crate::with_gil_entry_nopanic!(_py, {
                    assert!(has_capability(_py, "fs.read"));
                    let bootstrap = crate::with_gil_entry_nopanic!(py, {
                        sys_bootstrap_state_from_module_file(py, Some(bootstrap_module_file()))
                    });
                    assert!(bootstrap.include_cwd);
                    let missing =
                        importlib_resolve_spec_in_paths(_py, "nativepkg._native", &[], true)
                            .expect("empty explicit paths");
                    assert!(
                        missing.is_none(),
                        "bootstrap roots must not supply a raw path miss"
                    );
                    let stale = importlib_resolve_spec_in_paths(
                        _py,
                        "nativepkg._native",
                        &stale_search_paths,
                        true,
                    )
                    .expect("explicit stale path")
                    .expect("stale extension");
                    assert_eq!(stale.origin.as_deref(), Some(stale_ext_text.as_str()));
                    let payload_result = importlib_resolve_spec_in_paths(
                        _py,
                        "nativepkg._native",
                        &[staged_pkg.to_string_lossy().into_owned()],
                        true,
                    );
                    let payload = match payload_result {
                        Ok(Some(payload)) => payload,
                        Ok(None) => panic!("expected staged extension payload"),
                        Err(_) => panic!(
                            "find spec payload failed: {:?}",
                            pending_exception_kind_and_message(_py)
                        ),
                    };
                    assert_eq!(payload.origin.as_deref(), Some(staged_ext_text.as_str()));
                    assert_eq!(payload.loader_kind, "extension");
                });
            },
        );
    });

    std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
}

#[test]
fn importlib_find_in_path_resolves_sourceless_bytecode() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!(
        "molt_importlib_bytecode_spec_{}_{}",
        std::process::id(),
        stamp
    ));
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    std::fs::write(tmp.join("bcmod.pyc"), b"bytecode").expect("write module bytecode");

    let pkg_dir = tmp.join("bcpkg");
    std::fs::create_dir_all(&pkg_dir).expect("create package dir");
    std::fs::write(pkg_dir.join("__init__.pyc"), b"bytecode").expect("write package bytecode");

    let search_paths = vec![tmp.to_string_lossy().into_owned()];
    let module = importlib_find_in_path("bcmod", &search_paths, false).expect("module spec");
    assert_eq!(module.loader_kind, "bytecode");
    assert_eq!(module.cached, None);
    assert!(
        module
            .origin
            .as_deref()
            .unwrap_or("")
            .ends_with("bcmod.pyc")
    );

    let package = importlib_find_in_path("bcpkg", &search_paths, false).expect("package spec");
    assert_eq!(package.loader_kind, "bytecode");
    assert!(package.is_package);
    assert_eq!(
        package.submodule_search_locations,
        Some(vec![pkg_dir.to_string_lossy().into_owned()])
    );

    std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
}

#[cfg(feature = "stdlib_archive")]
#[test]
#[cfg_attr(miri, ignore)]
fn importlib_find_in_path_resolves_zip_source_module_and_package() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!(
        "molt_importlib_zip_source_spec_{}_{}",
        std::process::id(),
        stamp
    ));
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let archive = tmp.join("mods.zip");
    let file = std::fs::File::create(&archive).expect("create zip file");
    let mut writer = zip::ZipWriter::new(file);
    let options: zip::write::SimpleFileOptions =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    writer
        .start_file("zipmod.py", options)
        .expect("start module zip entry");
    writer
        .write_all(b"value = 11\n")
        .expect("write module source");
    writer
        .start_file("zpkg/__init__.py", options)
        .expect("start package zip entry");
    writer
        .write_all(b"flag = 7\n")
        .expect("write package source");
    writer.finish().expect("finish zip file");

    let archive_text = archive.to_string_lossy().into_owned();
    let search_paths = vec![archive_text.clone()];
    let module = importlib_find_in_path("zipmod", &search_paths, false).expect("module spec");
    assert_eq!(module.loader_kind, "zip_source");
    assert_eq!(module.zip_archive, Some(archive_text.clone()));
    assert_eq!(module.zip_inner_path, Some("zipmod.py".to_string()));
    assert!(
        module
            .origin
            .as_deref()
            .unwrap_or("")
            .ends_with("mods.zip/zipmod.py")
    );

    let package = importlib_find_in_path("zpkg", &search_paths, false).expect("package spec");
    assert_eq!(package.loader_kind, "zip_source");
    assert!(package.is_package);
    assert_eq!(package.zip_archive, Some(archive_text.clone()));
    assert_eq!(package.zip_inner_path, Some("zpkg/__init__.py".to_string()));
    assert_eq!(
        package.submodule_search_locations,
        Some(vec![format!("{archive_text}/zpkg")])
    );

    std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
}

#[cfg(feature = "stdlib_archive")]
#[test]
#[cfg_attr(miri, ignore)]
fn importlib_zip_source_exec_payload_reads_source_and_resolution() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!(
        "molt_importlib_zip_exec_payload_{}_{}",
        std::process::id(),
        stamp
    ));
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let archive = tmp.join("mods.zip");
    let file = std::fs::File::create(&archive).expect("create zip file");
    let mut writer = zip::ZipWriter::new(file);
    let options: zip::write::SimpleFileOptions =
        zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
    writer
        .start_file("zipmod.py", options)
        .expect("start module zip entry");
    writer
        .write_all(b"value = 41\n")
        .expect("write module source");
    writer.finish().expect("finish zip file");

    let archive_text = archive.to_string_lossy().into_owned();
    let payload = importlib_zip_source_exec_payload("zipmod", &archive_text, "zipmod.py", false)
        .expect("build zip source exec payload");
    assert!(!payload.is_package);
    assert_eq!(payload.module_package, "");
    assert_eq!(payload.package_root, None);
    assert!(payload.origin.ends_with("mods.zip/zipmod.py"));
    let text = String::from_utf8(payload.source.clone()).expect("decode source text");
    assert!(text.contains("value = 41"));

    std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
}

#[test]
fn importlib_search_paths_includes_bootstrap_roots_and_stdlib_candidates() {
    let sep = if sys_platform_str().starts_with("win") {
        ';'
    } else {
        ':'
    };
    let module_roots = format!("vendor{sep}extra");
    with_env_state(
        &[
            ("PYTHONPATH", ""),
            ("MOLT_MODULE_ROOTS", &module_roots),
            ("MOLT_CAPABILITY_TIER", "full"),
            ("PWD", "/tmp/bootstrap_pwd"),
        ],
        || {
            let resolved = crate::with_gil_entry_nopanic!(py, {
                importlib_search_paths(py, &["src".to_string()], Some(bootstrap_module_file()))
            });
            assert!(resolved.iter().any(|entry| entry == "src"));
            let path_sep = if sys_platform_str().starts_with("win") {
                '\\'
            } else {
                '/'
            };
            let expected_vendor =
                bootstrap_resolve_path_entry("vendor", "/tmp/bootstrap_pwd", path_sep);
            let expected_extra =
                bootstrap_resolve_path_entry("extra", "/tmp/bootstrap_pwd", path_sep);
            assert!(resolved.iter().any(|entry| entry == &expected_vendor));
            assert!(resolved.iter().any(|entry| entry == &expected_extra));
            assert!(resolved.iter().any(|entry| {
                entry.ends_with("/molt/stdlib") || entry.ends_with("\\molt\\stdlib")
            }));
            assert!(
                resolved
                    .iter()
                    .any(|entry| entry == &expected_stdlib_root())
            );
        },
    );
}

#[test]
fn importlib_namespace_paths_finds_namespace_dirs() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!(
        "molt_importlib_namespace_paths_{}_{}",
        std::process::id(),
        stamp
    ));
    let base_one = tmp.join("base_one");
    let base_two = tmp.join("base_two");
    let ns_one = base_one.join("nsdemo");
    let ns_two = base_two.join("nsdemo");
    std::fs::create_dir_all(&ns_one).expect("create namespace dir one");
    std::fs::create_dir_all(&ns_two).expect("create namespace dir two");
    let ns_one_text = ns_one.to_string_lossy().into_owned();
    let ns_two_text = ns_two.to_string_lossy().into_owned();
    let search_paths = vec![
        base_one.to_string_lossy().into_owned(),
        base_two.to_string_lossy().into_owned(),
    ];
    let resolved = crate::with_gil_entry_nopanic!(py, {
        importlib_namespace_paths(py, "nsdemo", &search_paths, Some(bootstrap_module_file()))
    });
    assert!(resolved.iter().any(|entry| entry == &ns_one_text));
    assert!(resolved.iter().any(|entry| entry == &ns_two_text));
    std::fs::remove_dir_all(&tmp).expect("cleanup temp dirs");
}

#[cfg(feature = "stdlib_archive")]
#[test]
#[cfg_attr(miri, ignore)]
fn importlib_namespace_paths_finds_zip_namespace_dirs() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!(
        "molt_importlib_namespace_zip_paths_{}_{}",
        std::process::id(),
        stamp
    ));
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let archive = tmp.join("mods.zip");
    let file = std::fs::File::create(&archive).expect("create zip file");
    let mut writer = zip::ZipWriter::new(file);
    let options: zip::write::SimpleFileOptions = zip::write::FileOptions::default();
    writer
        .start_file("nszip/pkg/mod.py", options)
        .expect("start namespace file");
    writer
        .write_all(b"value = 1\n")
        .expect("write namespace file");
    writer.finish().expect("finish zip archive");

    let archive_text = archive.to_string_lossy().into_owned();
    let expected = format!("{archive_text}/nszip/pkg");
    let resolved = crate::with_gil_entry_nopanic!(py, {
        importlib_namespace_paths(
            py,
            "nszip.pkg",
            &[archive_text],
            Some(bootstrap_module_file()),
        )
    });
    assert!(resolved.iter().any(|entry| entry == &expected));

    std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
}

#[test]
fn importlib_metadata_dist_paths_finds_dist_info_dirs() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!(
        "molt_importlib_metadata_dist_paths_{}_{}",
        std::process::id(),
        stamp
    ));
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let dist_info = tmp.join("pkgdemo-1.0.dist-info");
    let egg_info = tmp.join("otherpkg-2.0.egg-info");
    std::fs::create_dir_all(&dist_info).expect("create dist-info dir");
    std::fs::create_dir_all(&egg_info).expect("create egg-info dir");
    std::fs::write(tmp.join("not_a_dist-info"), "x").expect("create plain file");
    let dist_info_text = dist_info.to_string_lossy().into_owned();
    let egg_info_text = egg_info.to_string_lossy().into_owned();
    let resolved = crate::with_gil_entry_nopanic!(py, {
        importlib_metadata_dist_paths(
            py,
            &[tmp.to_string_lossy().into_owned()],
            Some(bootstrap_module_file()),
        )
    });
    assert!(resolved.iter().any(|entry| entry == &dist_info_text));
    assert!(resolved.iter().any(|entry| entry == &egg_info_text));
    assert!(
        !resolved
            .iter()
            .any(|entry| entry.ends_with("not_a_dist-info")),
        "non-dist-info file leaked into metadata path scan"
    );
    std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
}

#[test]
fn importlib_metadata_payload_parses_name_version_and_entry_points() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!(
        "molt_importlib_metadata_payload_{}_{}",
        std::process::id(),
        stamp
    ));
    let dist = tmp.join("demo_pkg-1.2.3.dist-info");
    std::fs::create_dir_all(&dist).expect("create dist-info dir");
    std::fs::write(
        dist.join("METADATA"),
        "Name: demo-pkg\nVersion: 1.2.3\nSummary: demo\nRequires-Python: >=3.12\nRequires-Dist: dep-one>=1\nRequires-Dist: dep-two; extra == \"dev\"\nProvides-Extra: dev\n",
    )
    .expect("write metadata");
    std::fs::write(
        dist.join("entry_points.txt"),
        "[console_scripts]\ndemo = demo_pkg:main\n",
    )
    .expect("write entry points");

    let payload = importlib_metadata_payload(&dist.to_string_lossy());
    assert_eq!(payload.name, "demo-pkg");
    assert_eq!(payload.version, "1.2.3");
    assert!(
        payload
            .metadata
            .iter()
            .any(|(key, value)| key == "Name" && value == "demo-pkg")
    );
    assert!(payload.entry_points.iter().any(|(name, value, group)| {
        name == "demo" && value == "demo_pkg:main" && group == "console_scripts"
    }));
    assert_eq!(payload.requires_python.as_deref(), Some(">=3.12"));
    assert_eq!(payload.requires_dist.len(), 2);
    assert!(
        payload
            .requires_dist
            .iter()
            .any(|value| value == "dep-one>=1")
    );
    assert!(
        payload
            .requires_dist
            .iter()
            .any(|value| value == "dep-two; extra == \"dev\"")
    );
    assert!(payload.provides_extra.iter().any(|value| value == "dev"));

    std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
}

#[cfg(feature = "stdlib_fs_extra")]
#[test]
fn importlib_metadata_record_payload_parses_rows() {
    let tmp = tempfile::Builder::new()
        .prefix("molt_importlib_metadata_record_payload_")
        .tempdir_in(std::env::temp_dir())
        .expect("create temp dir");
    let dist = tmp.path().join("demo_record-1.0.dist-info");
    std::fs::create_dir_all(&dist).expect("create dist-info dir");
    std::fs::write(
        dist.join("RECORD"),
        "demo_record/__init__.py,sha256=abc123,17\n\"demo_record/data,file.txt\",,\n",
    )
    .expect("write RECORD");

    let payload = importlib_metadata_record_payload(&dist.to_string_lossy());
    assert_eq!(payload.len(), 2);
    assert_eq!(payload[0].path, "demo_record/__init__.py");
    assert_eq!(payload[0].hash.as_deref(), Some("sha256=abc123"));
    assert_eq!(payload[0].size.as_deref(), Some("17"));
    assert_eq!(payload[1].path, "demo_record/data,file.txt");
    assert!(payload[1].hash.is_none());
    assert!(payload[1].size.is_none());
}

#[test]
fn importlib_metadata_packages_distributions_payload_aggregates_top_level() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!(
        "molt_importlib_metadata_packages_payload_{}_{}",
        std::process::id(),
        stamp
    ));
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let dist_one = tmp.join("demo_one-1.0.dist-info");
    let dist_two = tmp.join("demo_two-2.0.dist-info");
    std::fs::create_dir_all(&dist_one).expect("create dist one");
    std::fs::create_dir_all(&dist_two).expect("create dist two");
    std::fs::write(dist_one.join("METADATA"), "Name: demo-one\nVersion: 1.0\n")
        .expect("write metadata one");
    std::fs::write(dist_two.join("METADATA"), "Name: demo-two\nVersion: 2.0\n")
        .expect("write metadata two");
    std::fs::write(
        dist_one.join("top_level.txt"),
        "pkg_one\npkg_shared\npkg_shared\n",
    )
    .expect("write top_level one");
    std::fs::write(dist_two.join("top_level.txt"), "pkg_two\npkg_shared\n")
        .expect("write top_level two");

    let payload = crate::with_gil_entry_nopanic!(py, {
        importlib_metadata_packages_distributions_payload(
            py,
            &[tmp.to_string_lossy().into_owned()],
            Some(bootstrap_module_file()),
        )
    });
    let mapping: BTreeMap<String, Vec<String>> = payload.into_iter().collect();
    assert_eq!(mapping.get("pkg_one"), Some(&vec!["demo-one".to_string()]));
    assert_eq!(mapping.get("pkg_two"), Some(&vec!["demo-two".to_string()]));
    assert_eq!(
        mapping.get("pkg_shared"),
        Some(&vec!["demo-one".to_string(), "demo-two".to_string()])
    );

    std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
}

#[test]
fn importlib_bootstrap_payload_reports_resolved_search_paths_and_env_fields() {
    let sep = if sys_platform_str().starts_with("win") {
        ';'
    } else {
        ':'
    };
    let module_roots = format!("vendor{sep}extra");
    with_env_state(
        &[
            ("PYTHONPATH", "alpha"),
            ("MOLT_MODULE_ROOTS", &module_roots),
            ("VIRTUAL_ENV", ""),
            ("MOLT_CAPABILITY_TIER", "full"),
            ("PWD", "/tmp/bootstrap_pwd"),
        ],
        || {
            let payload = crate::with_gil_entry_nopanic!(py, {
                importlib_bootstrap_payload(py, &["src".to_string()], Some(bootstrap_module_file()))
            });
            let path_sep = if sys_platform_str().starts_with("win") {
                '\\'
            } else {
                '/'
            };
            let expected_vendor =
                bootstrap_resolve_path_entry("vendor", "/tmp/bootstrap_pwd", path_sep);
            assert!(
                payload
                    .resolved_search_paths
                    .iter()
                    .any(|entry| entry == "src")
            );
            assert!(
                payload
                    .resolved_search_paths
                    .iter()
                    .any(|entry| entry == &expected_stdlib_root())
            );
            assert!(payload.pythonpath_entries.is_empty());
            assert!(
                payload
                    .module_roots_entries
                    .iter()
                    .any(|entry| entry == &expected_vendor)
            );
            assert!(payload.venv_site_packages_entries.is_empty());
            assert!(payload.include_cwd);
            assert_eq!(payload.pwd, "/tmp/bootstrap_pwd");
        },
    );
}

#[test]
fn importlib_metadata_entry_points_payload_aggregates_dist_entry_points() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!(
        "molt_importlib_metadata_entry_points_payload_{}_{}",
        std::process::id(),
        stamp
    ));
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let dist_one = tmp.join("demo_one-1.0.dist-info");
    let dist_two = tmp.join("demo_two-2.0.dist-info");
    std::fs::create_dir_all(&dist_one).expect("create dist one");
    std::fs::create_dir_all(&dist_two).expect("create dist two");
    std::fs::write(
        dist_one.join("entry_points.txt"),
        "[console_scripts]\none = demo_one:main\n",
    )
    .expect("write entry_points one");
    std::fs::write(
        dist_two.join("entry_points.txt"),
        "[demo.group]\ntwo = demo_two:value\n",
    )
    .expect("write entry_points two");
    let payload = crate::with_gil_entry_nopanic!(py, {
        importlib_metadata_entry_points_payload(
            py,
            &[tmp.to_string_lossy().into_owned()],
            Some(bootstrap_module_file()),
        )
    });
    assert!(payload.iter().any(|(name, value, group)| name == "one"
        && value == "demo_one:main"
        && group == "console_scripts"));
    assert!(payload.iter().any(|(name, value, group)| name == "two"
        && value == "demo_two:value"
        && group == "demo.group"));
    std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
}

#[test]
fn importlib_metadata_entry_points_select_payload_filters_by_group_and_name() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!(
        "molt_importlib_metadata_entry_points_select_payload_{}_{}",
        std::process::id(),
        stamp
    ));
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let dist = tmp.join("demo_select-1.0.dist-info");
    std::fs::create_dir_all(&dist).expect("create dist");
    std::fs::write(
        dist.join("entry_points.txt"),
        "[console_scripts]\nalpha = demo:alpha\nbeta = demo:beta\n[demo.group]\nalpha = demo:value\n",
    )
    .expect("write entry points");
    let search_paths = vec![tmp.to_string_lossy().into_owned()];
    let group_filtered = crate::with_gil_entry_nopanic!(py, {
        importlib_metadata_entry_points_select_payload(
            py,
            &search_paths,
            Some(bootstrap_module_file()),
            Some("console_scripts"),
            None,
        )
    });
    assert_eq!(group_filtered.len(), 2);
    assert!(
        group_filtered
            .iter()
            .all(|(_, _, group)| group == "console_scripts")
    );
    let name_filtered = crate::with_gil_entry_nopanic!(py, {
        importlib_metadata_entry_points_select_payload(
            py,
            &search_paths,
            Some(bootstrap_module_file()),
            Some("console_scripts"),
            Some("beta"),
        )
    });
    assert_eq!(name_filtered.len(), 1);
    assert_eq!(name_filtered[0].0, "beta");
    std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
}

#[test]
fn importlib_metadata_entry_points_filter_payload_filters_by_value() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!(
        "molt_importlib_metadata_entry_points_filter_payload_{}_{}",
        std::process::id(),
        stamp
    ));
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let dist = tmp.join("demo_filter-1.0.dist-info");
    std::fs::create_dir_all(&dist).expect("create dist");
    std::fs::write(
        dist.join("entry_points.txt"),
        "[demo.group]\nalpha = demo:alpha\nbeta = demo:beta\n",
    )
    .expect("write entry points");
    let search_paths = vec![tmp.to_string_lossy().into_owned()];
    let filtered = crate::with_gil_entry_nopanic!(py, {
        importlib_metadata_entry_points_filter_payload(
            py,
            &search_paths,
            Some(bootstrap_module_file()),
            Some("demo.group"),
            None,
            Some("demo:beta"),
        )
    });
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].0, "beta");
    assert_eq!(filtered[0].1, "demo:beta");
    std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
}

#[test]
fn importlib_metadata_distributions_payload_aggregates_dist_payloads() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!(
        "molt_importlib_metadata_distributions_payload_{}_{}",
        std::process::id(),
        stamp
    ));
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let dist_one = tmp.join("demo_bulk_one-1.0.dist-info");
    let dist_two = tmp.join("demo_bulk_two-2.0.dist-info");
    std::fs::create_dir_all(&dist_one).expect("create dist one");
    std::fs::create_dir_all(&dist_two).expect("create dist two");
    std::fs::write(
        dist_one.join("METADATA"),
        "Name: demo-bulk-one\nVersion: 1.0\n",
    )
    .expect("write metadata one");
    std::fs::write(
        dist_two.join("METADATA"),
        "Name: demo-bulk-two\nVersion: 2.0\n",
    )
    .expect("write metadata two");

    let payloads = crate::with_gil_entry_nopanic!(py, {
        importlib_metadata_distributions_payload(
            py,
            &[tmp.to_string_lossy().into_owned()],
            Some(bootstrap_module_file()),
        )
    });
    assert_eq!(payloads.len(), 2);
    assert!(
        payloads
            .iter()
            .any(|payload| payload.name == "demo-bulk-one" && payload.version == "1.0")
    );
    assert!(
        payloads
            .iter()
            .any(|payload| payload.name == "demo-bulk-two" && payload.version == "2.0")
    );

    std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
}

#[test]
fn importlib_metadata_normalize_name_collapses_separator_runs() {
    assert_eq!(
        importlib_metadata_normalize_name("Demo__payload---pkg.name"),
        "demo-payload-pkg-name"
    );
    assert_eq!(
        importlib_metadata_normalize_name("alpha...beta___gamma"),
        "alpha-beta-gamma"
    );
}

fn loader_identity_class(py: &PyToken<'_>, name: &str, base: u64) -> u64 {
    let name = alloc_test_string_bits(py, name);
    let class = crate::molt_class_new(name);
    crate::molt_class_set_base(class, base);
    unsafe {
        crate::object::class_finish_definition(py, obj_from_bits(class).as_ptr().unwrap())
            .expect("seal loader test class");
    }
    dec_ref_bits(py, name);
    assert!(!exception_pending(py));
    class
}

fn loader_identity_setattr(py: &PyToken<'_>, object: u64, name: &[u8], value: u64) {
    let result = unsafe {
        crate::c_api::molt_object_setattr_bytes(object, name.as_ptr(), name.len() as u64, value)
    };
    assert_eq!(result, 0, "{:?}", pending_exception_kind_and_message(py));
    assert!(!exception_pending(py));
}

fn loader_identity_class_namespace(py: &PyToken<'_>, class: u64, name: &[u8], value: u64) {
    // Seed the namespace as a class body does. Setting Type.__class__ through
    // setattr would instead request a metaclass change.
    let name_bits = crate::attr_name_bits_from_bytes(py, name).unwrap();
    unsafe {
        let ptr = obj_from_bits(class).as_ptr().unwrap();
        let dict = obj_from_bits(crate::object::layout::class_dict_bits(ptr))
            .as_ptr()
            .unwrap();
        assert!(dict_set_in_place(py, dict, name_bits, value));
        crate::object::layout::class_bump_layout_version(ptr);
    }
    dec_ref_bits(py, name_bits);
    assert!(!exception_pending(py));
}

extern "C" fn loader_identity_descriptor_failure(_self: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        raise_exception::<_>(py, "RuntimeError", "loader identity descriptor called")
    })
}

fn loader_identity_failing_property(py: &PyToken<'_>) -> u64 {
    let function = crate::builtins::functions::alloc_runtime_function_obj(
        py,
        crate::builtins::functions::runtime_fn_addr(
            "loader_identity_descriptor_failure",
            loader_identity_descriptor_failure as *const (),
        ),
        1,
    );
    assert!(!function.is_null());
    let function_bits = MoltObject::from_ptr(function).bits();
    let property = crate::object::builders::alloc_property_obj(
        py,
        function_bits,
        MoltObject::none().bits(),
        MoltObject::none().bits(),
    );
    dec_ref_bits(py, function_bits);
    assert!(!property.is_null());
    MoltObject::from_ptr(property).bits()
}

#[test]
fn extension_loader_admission_uses_class_declaration_not_names_or_descriptors() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        use crate::object::class_storage::{ClassDeclaration, class_declares};
        let spec = extension_spec_bits_for_tests(py, "role_extension", "extension.payload");
        for name in ["ExtensionFileLoader", "CustomExtensionFileLoaderSuffix"] {
            let class = loader_identity_class(py, name, builtin_classes(py).object);
            let property = loader_identity_failing_property(py);
            loader_identity_class_namespace(py, class, b"__class__", property);
            dec_ref_bits(py, property);
            let loader = unsafe { call_callable0(py, class) };
            loader_identity_setattr(py, spec, b"loader", loader);
            assert_eq!(
                importlib_extension_spec_target(py, "role_extension", spec),
                Ok(None)
            );
            assert!(!exception_pending(py), "__class__ must not be evaluated");
            dec_ref_bits(py, loader);
            dec_ref_bits(py, class);
        }

        // There is deliberately no module/class registration. The actual class
        // and its MRO own the role even when no facade publishes either class.
        let base = loader_identity_class(py, "Ordinary", builtin_classes(py).object);
        let child = loader_identity_class(py, "UnrelatedName", base);
        let loader = unsafe { call_callable0(py, child) };
        loader_identity_setattr(py, spec, b"loader", loader);
        assert_eq!(
            importlib_extension_spec_target(py, "role_extension", spec),
            Ok(None)
        );
        let base_ptr = obj_from_bits(base).as_ptr().unwrap();
        let before = unsafe { (*header_from_obj_ptr(base_ptr)).ref_count_snapshot() };
        for _ in 0..2 {
            let declared = molt_importlib_extension_loader_type_declare(base);
            assert!(obj_from_bits(declared).is_none());
            assert!(!exception_pending(py));
        }
        assert_eq!(
            unsafe { (*header_from_obj_ptr(base_ptr)).ref_count_snapshot() },
            before
        );
        assert!(unsafe { class_declares(base_ptr, ClassDeclaration::ExtensionLoader) });
        assert!(
            !unsafe {
                class_declares(
                    obj_from_bits(child).as_ptr().unwrap(),
                    ClassDeclaration::ExtensionLoader,
                )
            },
            "inherited facts must not be copied into descendants"
        );
        let new_name = alloc_test_string_bits(py, "Renamed");
        loader_identity_setattr(py, base, b"__name__", new_name);
        dec_ref_bits(py, new_name);
        assert_eq!(
            importlib_extension_spec_target(py, "role_extension", spec),
            Ok(Some((
                "role_extension".to_string(),
                "extension.payload".to_string()
            )))
        );
        loader_identity_setattr(py, spec, b"origin", MoltObject::none().bits());
        assert!(importlib_extension_spec_target(py, "role_extension", spec).is_err());
        assert_pending_exception_contains(
            py,
            "ImportError",
            &["extension module path must point to a file"],
        );
        for bits in [spec, loader, child, base] {
            dec_ref_bits(py, bits);
        }
    });
}

#[test]
fn extension_loader_declaration_preserves_nonstandard_artifact_manifest_admission() {
    crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
        clear_extension_metadata_validation_cache();
        let tmp = extension_boundary_temp_dir("molt_extension_declared_nonstandard");
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let path = tmp.join("extension.payload");
        std::fs::write(&path, b"declared-loader-artifact").expect("write artifact");
        let path_text = path.to_string_lossy().into_owned();
        crate::with_gil_entry_nopanic!(py, {
            let class = loader_identity_class(py, "Unrelated", builtin_classes(py).object);
            molt_importlib_extension_loader_type_declare(class);
            let loader = unsafe { call_callable0(py, class) };
            let spec = extension_spec_bits_for_tests(py, "role_extension", &path_text);
            loader_identity_setattr(py, spec, b"loader", loader);
            assert!(
                importlib_enforce_extension_spec_object_boundary(py, "role_extension", spec)
                    .is_err()
            );
            assert_pending_exception_contains(py, "ImportError", &["extension metadata missing"]);
            write_valid_extension_manifest(
                &tmp.join("extension_manifest.json"),
                "role_extension",
                "extension.payload",
                &importlib_sha256_file(&path_text).expect("hash extension artifact"),
            );
            assert!(
                importlib_enforce_extension_spec_object_boundary(py, "role_extension", spec)
                    .is_ok()
            );
            assert!(!exception_pending(py));
            // Same loader identity never grants permission to stale bytes.
            std::fs::write(&path, b"changed-artifact").expect("mutate artifact");
            assert!(
                importlib_enforce_extension_spec_object_boundary(py, "role_extension", spec)
                    .is_err()
            );
            assert_pending_exception_contains(py, "ImportError", &["extension checksum mismatch"]);
            for bits in [spec, loader, class] {
                dec_ref_bits(py, bits);
            }
        });
        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    });
}

#[test]
fn extension_spec_origin_suffix_and_descriptor_errors_remain_independent_of_loader_role() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        for path in [
            "extension.pyd",
            "extension.so",
            "extensions.zip/extension.dll",
            "extension.dylib",
            "MODULE.PyD",
            "λ/extension.So",
            "archive.whl/pkg/EXTENSION.DLL",
            "outer.zip/inner.egg/extension.DYLIB/",
            "archive.zip\\extension.SO\\",
        ] {
            let spec = extension_spec_bits_for_tests(py, "role_extension", path);
            assert_eq!(
                importlib_extension_spec_target(py, "role_extension", spec),
                Ok(Some(("role_extension".to_string(), path.to_string())))
            );
            dec_ref_bits(py, spec);
        }
        for path in [
            "",
            "so",
            "extension.py",
            "extension.so.py",
            "extension.so/",
            "extension.dll\\",
            "archive.zip/extension.so.py",
            "archive.zip/extension.py/",
            "archive.SO.zip",
        ] {
            let spec = extension_spec_bits_for_tests(py, "role_extension", path);
            assert_eq!(
                importlib_extension_spec_target(py, "role_extension", spec),
                Ok(None)
            );
            dec_ref_bits(py, spec);
        }
        let class = loader_identity_class(py, "Spec", builtin_classes(py).object);
        let property = loader_identity_failing_property(py);
        loader_identity_setattr(py, class, b"loader", property);
        dec_ref_bits(py, property);
        let spec = unsafe { call_callable0(py, class) };
        let origin = alloc_test_string_bits(py, "extension.payload");
        loader_identity_setattr(py, spec, b"origin", origin);
        dec_ref_bits(py, origin);
        assert!(importlib_extension_spec_target(py, "role_extension", spec).is_err());
        assert_pending_exception_contains(
            py,
            "RuntimeError",
            &["loader identity descriptor called"],
        );
        let result = molt_importlib_extension_loader_type_declare(MoltObject::none().bits());
        assert!(obj_from_bits(result).is_none());
        assert_pending_exception_contains(
            py,
            "TypeError",
            &["extension loader declaration requires a type"],
        );
        for bits in [spec, class] {
            dec_ref_bits(py, bits);
        }
    });
}

#[cfg(not(target_arch = "wasm32"))]
mod resolver_admission_tests {
    use super::*;
    use std::cell::Cell;

    use crate::test_support::cargo_test_artifacts;

    thread_local! {
        static FINDER_CALLS: Cell<usize> = const { Cell::new(0) };
        // Borrowed only while the enclosing test owns the exception.
        static FINDER_ERROR: Cell<u64> = const { Cell::new(0) };
    }

    struct FinderState;

    impl Drop for FinderState {
        fn drop(&mut self) {
            FINDER_ERROR.set(0);
            FINDER_CALLS.set(0);
        }
    }

    extern "C" fn find_spec(_name: u64, _path: u64, _target: u64) -> u64 {
        FINDER_CALLS.set(FINDER_CALLS.get() + 1);
        let error = FINDER_ERROR.get();
        if error != 0 {
            crate::builtins::exceptions::molt_exception_set_last(error);
        }
        MoltObject::none().bits()
    }

    fn owner(bits: u64) -> crate::PtrDropGuard {
        crate::PtrDropGuard::new(obj_from_bits(bits).as_ptr().expect("owned test object"))
    }

    fn refs(bits: u64) -> u32 {
        unsafe {
            (*crate::header_from_obj_ptr(obj_from_bits(bits).as_ptr().unwrap()))
                .ref_count_snapshot()
        }
    }

    #[test]
    fn catalog_origin_match_uses_physical_paths_and_ignores_unmapped_build_origin() {
        let artifacts = cargo_test_artifacts::CargoTestArtifacts::new("catalog-origin-match")
            .expect("own physical source coordinates");
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        let directory = artifacts.path();
        let source = directory.join("provider").join("msilib.py");
        let external = directory.join("external").join("msilib.py");
        let boundary = directory.join("provider-suffix").join("msilib.py");
        for file in [&source, &external, &boundary] {
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, "marker = 42\n").unwrap();
        }
        crate::with_gil_entry_nopanic!(py, {
            let same = |left: &std::path::Path, right: &std::path::Path| {
                importlib_catalog_origin_matches(
                    py,
                    left.to_str().unwrap(),
                    right.to_str().unwrap(),
                )
                .expect("physical coordinate comparison")
            };
            assert!(same(&source, &source));
            assert!(!same(&external, &source));
            assert!(
                !same(&boundary, &source),
                "a lexical root prefix is not provenance"
            );
            assert!(!same(
                &source,
                &directory.join("unmapped-compiler-origin.py")
            ));
            assert!(!same(&directory.join("missing-candidate.py"), &source));
            #[cfg(unix)]
            {
                let external_alias = directory.join("provider-alias.py");
                std::os::unix::fs::symlink(&source, &external_alias).unwrap();
                assert!(
                    same(&external_alias, &source),
                    "actual source aliases retain identity"
                );
                let misleading_inside = source.parent().unwrap().join("external-alias.py");
                std::os::unix::fs::symlink(&external, &misleading_inside).unwrap();
                assert!(!same(&misleading_inside, &source));
            }
            assert!(!exception_pending(py));
        });
    }

    #[test]
    fn public_import_resolver_miss_and_error_do_not_load_extension_candidates() {
        let artifacts = cargo_test_artifacts::CargoTestArtifacts::new("import-resolver-admission")
            .expect("own import candidates within Cargo test artifacts");
        // These files are deliberately not libraries: reaching the raw loader
        // changes the observed exception from the resolver's result to ImportError.
        let suffixes = [
            ".so",
            ".pyd",
            ".dll",
            ".dylib",
            ".abi3.so",
            ".cpython-312-test.so",
            ".cpython-313t-test.so",
            ".cp314t-test.pyd",
        ];
        for (index, suffix) in suffixes.iter().enumerate() {
            std::fs::write(
                artifacts
                    .path()
                    .join(format!("molt_resolver_candidate_{index}{suffix}")),
                b"not an admitted dynamic library",
            )
            .expect("write extension candidate");
        }
        // A real admitted catalog body must not outrank the public finder.
        // The old compiled-first lane initialized g4_src before consulting it.
        crate::builtins::module_table::tests::install_test_registry();
        let names: Vec<String> = (0..suffixes.len())
            .map(|index| format!("molt_resolver_candidate_{index}"))
            .chain(std::iter::once("g4_src".to_owned()))
            .collect();
        crate::test_support::RuntimeTestTransaction::with_trusted_fresh_runtime(|| {
            crate::with_gil_entry_nopanic!(py, {
                let sys_name = alloc_test_string_bits(py, "sys");
                let _sys_name = owner(sys_name);
                let sys = crate::molt_module_new(sys_name);
                let _sys = owner(sys);
                crate::builtins::module_table::publish_interpreter_sys_for_test(py, sys);
                assert!(!exception_pending(py));
                let paths =
                    alloc_string_list_bits(py, &[artifacts.path().to_string_lossy().into_owned()])
                        .expect("sys.path fixture");
                let _paths = owner(paths);
                loader_identity_setattr(py, sys, b"path", paths);
                let modules = importlib_runtime_modules_bits(py).expect("public sys.modules");
                let _modules = owner(modules);
                let modules_ptr = obj_from_bits(modules).as_ptr().unwrap();
                let util_name = alloc_test_string_bits(py, "importlib.util");
                let _util_name = owner(util_name);
                let util = crate::molt_module_new(util_name);
                let _util = owner(util);
                let function_ptr = crate::builtins::functions::alloc_runtime_function_obj(
                    py,
                    crate::builtins::functions::runtime_fn_addr(
                        "import_resolver_admission_find_spec",
                        find_spec as *const (),
                    ),
                    3,
                );
                assert!(!function_ptr.is_null());
                let finder = MoltObject::from_ptr(function_ptr).bits();
                let _finder = owner(finder);
                loader_identity_setattr(py, util, b"find_spec", finder);
                let meta = alloc_list(py, &[util]);
                assert!(!meta.is_null());
                let meta = MoltObject::from_ptr(meta).bits();
                loader_identity_setattr(py, sys, b"meta_path", meta);
                dec_ref_bits(py, meta);
                unsafe {
                    assert!(dict_set_in_place(py, modules_ptr, util_name, util));
                }
                assert!(!exception_pending(py));
                let _ = raise_exception::<u64>(py, "RuntimeError", "finder's exact exception");
                let original = crate::builtins::exceptions::ExceptionValue::adopt(
                    py,
                    molt_exception_last_pending(),
                );
                clear_exception(py);
                // Reset borrowed callback state before original's owner drops, including unwind.
                let _finder_state = FinderState;
                FINDER_CALLS.set(0);
                FINDER_ERROR.set(0);
                let baseline = [
                    refs(sys),
                    refs(paths),
                    refs(modules),
                    refs(util),
                    refs(finder),
                ];
                for name_text in &names {
                    let name = alloc_test_string_bits(py, name_text);
                    let _name = owner(name);
                    for fail in [false, true] {
                        FINDER_ERROR.set(if fail { original.bits() } else { 0 });
                        let calls = FINDER_CALLS.get();
                        let result = molt_importlib_import_module(name, MoltObject::none().bits());
                        assert!(obj_from_bits(result).is_none());
                        assert_eq!(
                            FINDER_CALLS.get(),
                            calls + 1,
                            "finder is consulted exactly once"
                        );
                        if fail {
                            let raised = crate::builtins::exceptions::ExceptionValue::adopt(
                                py,
                                molt_exception_last_pending(),
                            );
                            assert_eq!(
                                raised.bits(),
                                original.bits(),
                                "propagate the original finder error"
                            );
                        } else {
                            assert_eq!(
                                pending_exception_kind_and_message(py),
                                Some((
                                    "ModuleNotFoundError".to_owned(),
                                    format!("No module named '{name_text}'"),
                                ))
                            );
                            let raised = crate::builtins::exceptions::ExceptionValue::adopt(
                                py,
                                molt_exception_last_pending(),
                            );
                            clear_exception(py);
                            for (field, expected) in [
                                (b"name".as_slice(), name),
                                (b"path".as_slice(), MoltObject::none().bits()),
                                (b"name_from".as_slice(), MoltObject::none().bits()),
                            ] {
                                let key =
                                    alloc_test_string_bits(py, std::str::from_utf8(field).unwrap());
                                let value = crate::builtins::attributes::get_attr_name(
                                    py,
                                    raised.bits(),
                                    key,
                                )
                                .expect("public import failure metadata");
                                assert_eq!(
                                    value, expected,
                                    "finder miss owns the admitted name and default path"
                                );
                                dec_ref_bits(py, value);
                                dec_ref_bits(py, key);
                            }
                        }
                        clear_exception(py);
                        assert_eq!(
                            unsafe {
                                dict_get_in_place(py, modules_ptr, name)
                                    .expect("dictionary lookup succeeds")
                            },
                            None,
                            "failed import must not publish a module"
                        );
                        assert_eq!(
                            [
                                refs(sys),
                                refs(paths),
                                refs(modules),
                                refs(util),
                                refs(finder)
                            ],
                            baseline,
                            "import must release every temporary support owner"
                        );
                    }
                    // Public cache authority still wins, even while the finder would raise.
                    let cached = crate::molt_module_new(name);
                    let _cached = owner(cached);
                    unsafe {
                        assert!(dict_set_in_place(py, modules_ptr, name, cached));
                    }
                    assert!(!exception_pending(py));
                    let cached_refs = refs(cached);
                    let calls = FINDER_CALLS.get();
                    let result = molt_importlib_import_module(name, MoltObject::none().bits());
                    assert_eq!(result, cached);
                    let result_owner = owner(result);
                    assert!(!exception_pending(py));
                    assert_eq!(FINDER_CALLS.get(), calls);
                    assert_eq!(
                        refs(cached),
                        cached_refs + 1,
                        "public import returns one owner"
                    );
                    drop(result_owner);
                    assert_eq!(refs(cached), cached_refs);
                }
                FINDER_ERROR.set(0);
            });
        });
    }
}

#[test]
fn environment_mapping_owns_exact_objects_and_process_only_writes_do_not_publish() {
    crate::test_support::RuntimeTestTransaction::with_explicit_capabilities(
        "env.read,env.write",
        |py| {
            let raw_key = std::ffi::OsStr::new("__MOLT_ENV_OBJECT_MAPPING_TEST__");
            let original = std::env::var_os(raw_key);
            let key = alloc_test_string_bits(py, raw_key.to_str().unwrap());
            let first = alloc_test_string_bits(py, "mapping");
            let second = alloc_test_string_bits(py, "process-only");
            #[cfg(not(windows))]
            let (encoded_key, encoded_value) = (
                MoltObject::from_ptr(alloc_bytes(py, raw_key.as_encoded_bytes())).bits(),
                MoltObject::from_ptr(alloc_bytes(py, b"mapping")).bits(),
            );
            #[cfg(windows)]
            let (encoded_key, encoded_value) = {
                inc_ref_bits(py, key);
                inc_ref_bits(py, first);
                (key, first)
            };
            molt_env_mapping_set(encoded_key, encoded_value);
            assert!(!exception_pending(py));
            assert_eq!(
                std::env::var_os(raw_key).as_deref(),
                Some(std::ffi::OsStr::new("mapping"))
            );
            let observed = molt_env_mapping_get(encoded_key);
            assert_eq!(
                observed, encoded_value,
                "get transfers the exact stored object"
            );
            dec_ref_bits(py, observed);
            molt_env_putenv(key, second);
            assert!(!exception_pending(py));
            assert_eq!(
                std::env::var_os(raw_key).as_deref(),
                Some(std::ffi::OsStr::new("process-only"))
            );
            let observed = molt_env_mapping_get(encoded_key);
            assert_eq!(observed, encoded_value);
            dec_ref_bits(py, observed);
            molt_env_unsetenv(key);
            assert!(!exception_pending(py));
            let observed = molt_env_mapping_get(encoded_key);
            assert_eq!(observed, encoded_value);
            dec_ref_bits(py, observed);
            molt_env_mapping_delete(encoded_key);
            assert!(!exception_pending(py));
            process_env_set(raw_key, original.as_deref()).expect("restore process environment");
            for value in [key, first, second, encoded_key, encoded_value] {
                dec_ref_bits(py, value);
            }
        },
    );
}

#[test]
fn environment_mapping_is_lazy_and_terminal_retirement_precedes_direct_mutation() {
    crate::test_support::RuntimeTestTransaction::with_explicit_capabilities("env.write", |py| {
        assert_eq!(
            platform_state(py)
                .environment_mapping
                .load(Ordering::Acquire),
            0
        );
        molt_env_mapping_init();
        assert!(!exception_pending(py));
        let root = platform_state(py)
            .environment_mapping
            .load(Ordering::Acquire);
        assert_eq!(
            unsafe { object_type_id(obj_from_bits(root).as_ptr().unwrap()) },
            TYPE_ID_DICT
        );
        assert!(platform_clear_runtime_state(py, runtime_state(py)));
        assert_eq!(
            platform_state(py)
                .environment_mapping
                .load(Ordering::Acquire),
            MoltObject::none().bits()
        );
        let key = alloc_test_string_bits(py, "__MOLT_ENV_RETIRED_TEST__");
        let value = alloc_test_string_bits(py, "must-not-write");
        molt_env_putenv(key, value);
        assert!(exception_pending(py));
        assert_eq!(std::env::var_os("__MOLT_ENV_RETIRED_TEST__"), None);
        crate::molt_exception_clear();
        dec_ref_bits(py, key);
        dec_ref_bits(py, value);
    });
}

static ENVIRONMENT_FSPATH_CALLS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
extern "C" fn denied_environment_fspath(_self: u64) -> u64 {
    ENVIRONMENT_FSPATH_CALLS.fetch_add(1, Ordering::Relaxed);
    crate::with_gil_entry_nopanic!(py, {
        alloc_test_string_bits(py, "__MOLT_ENV_DENIED_CALLBACK__")
    })
}

#[test]
fn environment_intrinsic_admission_precedes_callbacks_and_operand_conversion() {
    crate::test_support::RuntimeTestTransaction::with_explicit_capabilities("", |py| {
        let function = crate::builtins::functions::alloc_runtime_function_obj(
            py,
            crate::builtins::functions::runtime_fn_addr(
                "denied_environment_fspath",
                denied_environment_fspath as *const (),
            ),
            1,
        );
        assert!(!function.is_null());
        let function = MoltObject::from_ptr(function).bits();
        let class = loader_identity_class(py, "DeniedEnvironmentPath", builtin_classes(py).object);
        loader_identity_class_namespace(py, class, b"__fspath__", function);
        loader_identity_class_namespace(py, class, b"__hash__", function);
        let operand = unsafe { call_callable0(py, class) };
        assert!(!exception_pending(py));
        ENVIRONMENT_FSPATH_CALLS.store(0, Ordering::Relaxed);
        let before = std::env::var_os("__MOLT_ENV_DENIED_CALLBACK__");
        for operand in [operand, MoltObject::from_int(42).bits()] {
            for operation in [
                OperationId::EnvGet,
                OperationId::EnvSet,
                OperationId::EnvUnset,
                OperationId::EnvPutenv,
                OperationId::EnvUnsetenv,
            ] {
                match operation {
                    OperationId::EnvGet => molt_env_mapping_get(operand),
                    OperationId::EnvSet => molt_env_mapping_set(operand, operand),
                    OperationId::EnvUnset => molt_env_mapping_delete(operand),
                    OperationId::EnvPutenv => molt_env_putenv(operand, operand),
                    OperationId::EnvUnsetenv => molt_env_unsetenv(operand),
                    _ => unreachable!(),
                };
                assert!(exception_pending(py));
                let required = if operation == OperationId::EnvGet {
                    "env.read"
                } else {
                    "env.write"
                };
                assert_eq!(
                    pending_exception_kind_and_message(py),
                    Some((
                        "PermissionError".to_owned(),
                        format!(
                            "missing [{required}] capabilities for '{}'",
                            operation.as_str()
                        )
                    ))
                );
                crate::molt_exception_clear();
                assert_eq!(ENVIRONMENT_FSPATH_CALLS.load(Ordering::Relaxed), 0);
                assert_eq!(std::env::var_os("__MOLT_ENV_DENIED_CALLBACK__"), before);
                assert_eq!(
                    platform_state(py)
                        .environment_mapping
                        .load(Ordering::Acquire),
                    0,
                    "denied access cannot initialize the secret mapping"
                );
            }
        }
        for operation in [
            molt_env_mapping_keys as extern "C" fn() -> u64,
            molt_env_mapping_len,
            molt_env_mapping_items_iter,
        ] {
            operation();
            assert!(exception_pending(py));
            crate::molt_exception_clear();
            assert_eq!(
                platform_state(py)
                    .environment_mapping
                    .load(Ordering::Acquire),
                0
            );
        }
        for bits in [operand, class, function] {
            dec_ref_bits(py, bits);
        }
    });
}

static METADATA_CACHE_READS: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
static METADATA_CACHE_FAIL: std::sync::atomic::AtomicBool =
    std::sync::atomic::AtomicBool::new(false);

extern "C" fn metadata_observed_cached(_spec: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        METADATA_CACHE_READS.fetch_add(1, Ordering::SeqCst);
        if METADATA_CACHE_FAIL.load(Ordering::SeqCst) {
            raise_exception::<_>(py, "RuntimeError", "cached getter ran")
        } else {
            MoltObject::from_int(431).bits()
        }
    })
}

#[test]
fn spec_metadata_cached_projection_obeys_target_version_and_factory_ownership() {
    use crate::builtins::exceptions::ExceptionValue;
    let transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let class = ExceptionValue::adopt(
            py,
            loader_identity_class(py, "ObservedMetadataSpec", builtin_classes(py).object),
        );
        let function = crate::builtins::functions::alloc_runtime_function_obj(
            py,
            crate::builtins::functions::runtime_fn_addr(
                "metadata_observed_cached",
                metadata_observed_cached as *const (),
            ),
            1,
        );
        assert!(!function.is_null());
        let function = ExceptionValue::adopt(py, MoltObject::from_ptr(function).bits());
        let property = crate::object::builders::alloc_property_obj(
            py,
            function.bits(),
            MoltObject::none().bits(),
            MoltObject::none().bits(),
        );
        assert!(!property.is_null());
        let property = ExceptionValue::adopt(py, MoltObject::from_ptr(property).bits());
        loader_identity_class_namespace(py, class.bits(), b"cached", property.bits());
        let spec = ExceptionValue::adopt(py, unsafe { call_callable0(py, class.bits()) });
        let name = ExceptionValue::adopt(py, alloc_test_string_bits(py, "metadata_target"));
        let parent = ExceptionValue::adopt(py, alloc_test_string_bits(py, ""));
        let origin = ExceptionValue::adopt(py, alloc_test_string_bits(py, "metadata-source"));
        for (key, value) in [
            (b"name".as_slice(), name.bits()),
            (b"loader", MoltObject::none().bits()),
            (b"parent", parent.bits()),
            (b"submodule_search_locations", MoltObject::none().bits()),
            (b"has_location", MoltObject::from_bool(true).bits()),
            (b"origin", origin.bits()),
        ] {
            loader_identity_setattr(py, spec.bits(), key, value);
        }
        let cache_key = intern_runtime_static_name(py, b"__cached__");
        assert_ne!(cache_key, 0);
        // Independent CPython controls: ordinary/factory-owned fields, reload
        // override, and a getter that must remain unobserved on 3.15+. These
        // policy cells are not target-version release acceptance.
        for minor in [12, 13, 14, 15] {
            transaction.with_target_python_minor(py, minor, || {
                for (preexisting, override_existing, fail) in [
                    (false, false, false),
                    (true, false, false),
                    (true, true, false),
                    (false, false, true),
                ] {
                    let module = ExceptionValue::adopt(py, unsafe {
                        call_callable0(py, crate::builtins::types::simplenamespace_class(py))
                    });
                    assert!(!exception_pending(py));
                    let existing = MoltObject::from_int(909).bits();
                    if preexisting {
                        loader_identity_setattr(py, module.bits(), b"__cached__", existing);
                    }
                    METADATA_CACHE_READS.store(0, Ordering::SeqCst);
                    METADATA_CACHE_FAIL.store(fail, Ordering::SeqCst);
                    let result = importlib_ffi::importlib_init_module_attrs_impl(
                        py,
                        spec.bits(),
                        module.bits(),
                        override_existing,
                    );
                    let observes_cache = minor < 15 && (!preexisting || override_existing);
                    if observes_cache && fail {
                        assert!(result.is_err());
                        assert_pending_exception_contains(
                            py,
                            "RuntimeError",
                            &["cached getter ran"],
                        );
                    } else {
                        assert!(result.is_ok());
                        assert!(!exception_pending(py));
                    }
                    assert_eq!(
                        METADATA_CACHE_READS.load(Ordering::SeqCst),
                        if observes_cache {
                            if fail { 1 } else { 2 }
                        } else {
                            0
                        }
                    );
                    let cached = getattr_optional_bits(py, module.bits(), cache_key).unwrap();
                    assert_eq!(
                        cached,
                        if preexisting && !observes_cache {
                            Some(existing)
                        } else if observes_cache && !fail {
                            Some(MoltObject::from_int(431).bits())
                        } else {
                            None
                        }
                    );
                    if let Some(value) = cached {
                        dec_ref_bits(py, value);
                    }
                    METADATA_CACHE_FAIL.store(false, Ordering::SeqCst);
                }
            });
        }
        let predicate_class = ExceptionValue::adopt(
            py,
            loader_identity_class(py, "CachedPredicateTrap", builtin_classes(py).object),
        );
        let predicate = ExceptionValue::adopt(py, loader_identity_failing_property(py));
        loader_identity_class_namespace(
            py,
            predicate_class.bits(),
            b"__cached__",
            predicate.bits(),
        );
        for minor in [14, 15] {
            transaction.with_target_python_minor(py, minor, || {
                let module = ExceptionValue::adopt(py, unsafe {
                    call_callable0(py, predicate_class.bits())
                });
                assert!(!exception_pending(py));
                METADATA_CACHE_READS.store(0, Ordering::SeqCst);
                let result = importlib_ffi::importlib_init_module_attrs_impl(
                    py,
                    spec.bits(),
                    module.bits(),
                    false,
                );
                if minor == 14 {
                    assert!(result.is_err());
                    assert_pending_exception_contains(
                        py,
                        "RuntimeError",
                        &["loader identity descriptor called"],
                    );
                } else {
                    assert!(result.is_ok());
                    assert!(!exception_pending(py));
                }
                assert_eq!(METADATA_CACHE_READS.load(Ordering::SeqCst), 0);
            });
        }
        assert!(!exception_pending(py));
    });
}

static EXEC_PRESENCE_READS: AtomicU64 = AtomicU64::new(0);
static EXEC_PRESENCE_CALLS: AtomicU64 = AtomicU64::new(0);
static EXEC_PRESENCE_FIRST: AtomicU64 = AtomicU64::new(0);
static EXEC_PRESENCE_SECOND: AtomicU64 = AtomicU64::new(0);
extern "C" fn execution_presence_first(_module: u64) -> u64 {
    EXEC_PRESENCE_CALLS.store(1, Ordering::SeqCst);
    MoltObject::none().bits()
}
extern "C" fn execution_presence_second(_module: u64) -> u64 {
    EXEC_PRESENCE_CALLS.store(2, Ordering::SeqCst);
    MoltObject::none().bits()
}
extern "C" fn execution_presence_getter(_loader: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let call = if EXEC_PRESENCE_READS.fetch_add(1, Ordering::SeqCst) == 0 {
            EXEC_PRESENCE_FIRST.load(Ordering::SeqCst)
        } else {
            EXEC_PRESENCE_SECOND.load(Ordering::SeqCst)
        };
        inc_ref_bits(py, call);
        call
    })
}

#[test]
fn existing_spec_exec_distinguishes_presence_probe_from_final_descriptor_call() {
    use crate::builtins::exceptions::ExceptionValue;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        let class = ExceptionValue::adopt(
            py,
            loader_identity_class(py, "ObservedExecutorLoader", builtin_classes(py).object),
        );
        let runtime_function = |name, address, arity| {
            let pointer = crate::builtins::functions::alloc_runtime_function_obj(
                py,
                crate::builtins::functions::runtime_fn_addr(name, address),
                arity,
            );
            assert!(!pointer.is_null());
            ExceptionValue::adopt(py, MoltObject::from_ptr(pointer).bits())
        };
        let first = runtime_function(
            "execution_presence_first",
            execution_presence_first as *const (),
            1,
        );
        let second = runtime_function(
            "execution_presence_second",
            execution_presence_second as *const (),
            1,
        );
        let getter = runtime_function(
            "execution_presence_getter",
            execution_presence_getter as *const (),
            1,
        );
        let property = crate::object::builders::alloc_property_obj(
            py,
            getter.bits(),
            MoltObject::none().bits(),
            MoltObject::none().bits(),
        );
        assert!(!property.is_null());
        let property = ExceptionValue::adopt(py, MoltObject::from_ptr(property).bits());
        loader_identity_class_namespace(py, class.bits(), b"exec_module", property.bits());
        let loader = ExceptionValue::adopt(py, unsafe { call_callable0(py, class.bits()) });
        let name = ExceptionValue::adopt(
            py,
            alloc_test_string_bits(py, "_molt_exec_descriptor_cutpoint"),
        );
        let spec = ExceptionValue::adopt(
            py,
            crate::builtins::types::alloc_module_spec(
                py,
                name.bits(),
                loader.bits(),
                MoltObject::none().bits(),
                MoltObject::from_bool(false).bits(),
            )
            .unwrap(),
        );
        let module = ExceptionValue::adopt(
            py,
            MoltObject::from_ptr(alloc_module_obj(py, name.bits())).bits(),
        );
        let dictionary = crate::builtins::module_table::module_dictionary(py).unwrap();
        importlib_dict_set_string_key(
            py,
            crate::ptr_from_bits(dictionary),
            name.bits(),
            module.bits(),
        )
        .unwrap();
        EXEC_PRESENCE_FIRST.store(first.bits(), Ordering::SeqCst);
        EXEC_PRESENCE_SECOND.store(second.bits(), Ordering::SeqCst);
        EXEC_PRESENCE_READS.store(0, Ordering::SeqCst);
        EXEC_PRESENCE_CALLS.store(0, Ordering::SeqCst);
        let result = importlib_spec_execution_transaction(
            py,
            "_molt_exec_descriptor_cutpoint",
            name.bits(),
            spec.bits(),
            crate::ptr_from_bits(dictionary),
            ImportlibSpecExecutionOptions {
                target: ImportlibSpecTarget::Exact(module.bits()),
                allow_load_module_fallback: false,
            },
        );
        EXEC_PRESENCE_FIRST.store(0, Ordering::SeqCst);
        EXEC_PRESENCE_SECOND.store(0, Ordering::SeqCst);
        let reads = EXEC_PRESENCE_READS.load(Ordering::SeqCst);
        let called = EXEC_PRESENCE_CALLS.load(Ordering::SeqCst);
        let result = result.unwrap();
        dec_ref_bits(py, result);
        assert!(
            unsafe { dict_del_in_place(py, crate::ptr_from_bits(dictionary), name.bits()) }
                .unwrap()
        );
        assert_eq!(
            reads, 2,
            "hasattr and invocation are separate descriptor reads"
        );
        assert_eq!(
            called, 2,
            "the final descriptor, never the presence-probe value, executes"
        );
        assert!(!exception_pending(py));
    });
}

static SPEC_PROTOCOL_MODE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static SPEC_PROTOCOL_TRUE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static SPEC_PROTOCOL_FALSE: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
static SPEC_PROTOCOL_SPEC: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static SPEC_PROTOCOL_REPLACEMENT: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
static SPEC_PROTOCOL_NAME: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
static SPEC_PROTOCOL_DICTIONARY: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
static SPEC_PROTOCOL_BODY_ERROR: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);
static SPEC_PROTOCOL_FLAG_ERROR: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

extern "C" fn spec_protocol_setattr(spec: u64, name: u64, value: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let flag = crate::attr_name_bits_from_bytes(py, b"_initializing").unwrap();
        // The setter receives a pinned Python str key; flag is an owned str.
        let is_flag = unsafe { crate::object::ops_compare::string_storage_equal(name, flag) };
        dec_ref_bits(py, flag);
        if is_flag {
            let initializing = is_truthy(py, obj_from_bits(value));
            if initializing {
                SPEC_PROTOCOL_TRUE.fetch_add(1, Ordering::SeqCst);
            } else {
                SPEC_PROTOCOL_FALSE.fetch_add(1, Ordering::SeqCst);
            }
            let mode = SPEC_PROTOCOL_MODE.load(Ordering::SeqCst);
            if (mode == 3 && initializing) || (mode == 4 && !initializing) {
                return crate::molt_exception_set_last(
                    SPEC_PROTOCOL_FLAG_ERROR.load(Ordering::SeqCst),
                );
            }
        }
        crate::molt_object_setattr(spec, name, value)
    })
}

extern "C" fn spec_protocol_create(_loader: u64, _spec: u64) -> u64 {
    MoltObject::none().bits()
}

extern "C" fn spec_protocol_exec(_loader: u64, module: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let mode = SPEC_PROTOCOL_MODE.load(Ordering::SeqCst);
        let spec = SPEC_PROTOCOL_SPEC.load(Ordering::SeqCst);
        assert_eq!(
            spec_is_initializing(
                py,
                spec,
                crate::builtins::module_table::ModuleImportPolicy::Python
            ),
            Ok(mode != 5)
        );
        let dictionary = crate::ptr_from_bits(SPEC_PROTOCOL_DICTIONARY.load(Ordering::SeqCst));
        let name = SPEC_PROTOCOL_NAME.load(Ordering::SeqCst);
        assert_eq!(
            unsafe { dict_get_in_place(py, dictionary, name) }.unwrap(),
            Some(module)
        );
        if matches!(mode, 1 | 4 | 5) {
            return crate::molt_exception_set_last(SPEC_PROTOCOL_BODY_ERROR.load(Ordering::SeqCst));
        }
        if mode == 2 {
            assert!(unsafe { dict_del_in_place(py, dictionary, name) }.unwrap());
        } else {
            loader_identity_setattr(
                py,
                module,
                b"__spec__",
                SPEC_PROTOCOL_REPLACEMENT.load(Ordering::SeqCst),
            );
        }
        MoltObject::none().bits()
    })
}

#[test]
fn spec_transaction_finalizes_original_and_selects_flag_body_and_completion_errors() {
    use crate::builtins::exceptions::{self, ExceptionFieldSlot};
    use crate::builtins::module_table::ModuleImportPolicy;
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        for mode in 0..6 {
            SPEC_PROTOCOL_MODE.store(mode, Ordering::SeqCst);
            SPEC_PROTOCOL_TRUE.store(0, Ordering::SeqCst);
            SPEC_PROTOCOL_FALSE.store(0, Ordering::SeqCst);
            let spec_base = crate::molt_importlib_module_spec_type();
            let spec_class = loader_identity_class(py, "FlagProtocolSpec", spec_base);
            let setter = crate::builtins::functions::alloc_runtime_function_obj(
                py,
                crate::builtins::functions::runtime_fn_addr(
                    "spec_protocol_setattr",
                    spec_protocol_setattr as *const (),
                ),
                3,
            );
            let setter = MoltObject::from_ptr(setter).bits();
            loader_identity_class_namespace(py, spec_class, b"__setattr__", setter);
            let loader_class =
                loader_identity_class(py, "FlagProtocolLoader", builtin_classes(py).object);
            let executor = crate::builtins::functions::alloc_runtime_function_obj(
                py,
                crate::builtins::functions::runtime_fn_addr(
                    "spec_protocol_exec",
                    spec_protocol_exec as *const (),
                ),
                2,
            );
            let executor = MoltObject::from_ptr(executor).bits();
            loader_identity_class_namespace(py, loader_class, b"exec_module", executor);
            let creator = crate::builtins::functions::alloc_runtime_function_obj(
                py,
                crate::builtins::functions::runtime_fn_addr(
                    "spec_protocol_create",
                    spec_protocol_create as *const (),
                ),
                2,
            );
            let creator = MoltObject::from_ptr(creator).bits();
            loader_identity_class_namespace(py, loader_class, b"create_module", creator);
            let loader = unsafe { call_callable0(py, loader_class) };
            let name = alloc_test_string_bits(py, "_molt_spec_flag_protocol");
            let spec = unsafe { call_callable2(py, spec_class, name, loader) };
            assert!(!exception_pending(py));
            let replacement =
                unsafe { call_callable0(py, crate::builtins::types::simplenamespace_class(py)) };
            crate::builtins::module_table::set_spec_initializing(py, replacement, true);
            let body_error =
                MoltObject::from_ptr(alloc_exception(py, "ValueError", "body identity")).bits();
            let flag_error =
                MoltObject::from_ptr(alloc_exception(py, "RuntimeError", "flag identity")).bits();
            let dictionary = crate::builtins::module_table::module_dictionary(py).unwrap();
            SPEC_PROTOCOL_SPEC.store(spec, Ordering::SeqCst);
            SPEC_PROTOCOL_REPLACEMENT.store(replacement, Ordering::SeqCst);
            SPEC_PROTOCOL_NAME.store(name, Ordering::SeqCst);
            SPEC_PROTOCOL_DICTIONARY.store(dictionary, Ordering::SeqCst);
            SPEC_PROTOCOL_BODY_ERROR.store(body_error, Ordering::SeqCst);
            SPEC_PROTOCOL_FLAG_ERROR.store(flag_error, Ordering::SeqCst);
            let existing = if mode == 5 {
                let module = importlib_ffi::importlib_module_from_spec_impl(py, spec);
                importlib_dict_set_string_key(py, crate::ptr_from_bits(dictionary), name, module)
                    .unwrap();
                Some(module)
            } else {
                None
            };
            let result = importlib_spec_execution_transaction(
                py,
                "_molt_spec_flag_protocol",
                name,
                spec,
                crate::ptr_from_bits(dictionary),
                ImportlibSpecExecutionOptions {
                    target: if mode == 5 {
                        ImportlibSpecTarget::Exact(
                            existing.expect("existing target in _exec control"),
                        )
                    } else {
                        ImportlibSpecTarget::Fresh
                    },
                    allow_load_module_fallback: false,
                },
            );
            assert_eq!(
                SPEC_PROTOCOL_TRUE.load(Ordering::SeqCst),
                usize::from(mode != 5)
            );
            assert_eq!(
                SPEC_PROTOCOL_FALSE.load(Ordering::SeqCst),
                usize::from(mode != 3 && mode != 5)
            );
            if mode == 0 {
                let module = result.unwrap();
                assert!(!exception_pending(py));
                assert_eq!(
                    spec_is_initializing(py, spec, ModuleImportPolicy::Python),
                    Ok(false)
                );
                assert_eq!(
                    spec_is_initializing(py, replacement, ModuleImportPolicy::Python),
                    Ok(true)
                );
                dec_ref_bits(py, module);
            } else {
                assert!(result.is_err());
                let selected = exceptions::molt_exception_last_pending();
                if mode == 2 {
                    assert!(exceptions::exception_matches_builtin_name(
                        py, selected, "KeyError"
                    ));
                } else {
                    assert_eq!(
                        selected,
                        if matches!(mode, 3 | 4) {
                            flag_error
                        } else {
                            body_error
                        }
                    );
                }
                if mode == 4 {
                    let context =
                        exceptions::exception_field(py, flag_error, ExceptionFieldSlot::Context)
                            .unwrap();
                    assert_eq!(context.bits(), body_error);
                }
                crate::clear_exception(py);
                dec_ref_bits(py, selected);
            }
            let public =
                unsafe { dict_get_in_place(py, crate::ptr_from_bits(dictionary), name) }.unwrap();
            assert_eq!(public.is_some(), matches!(mode, 0 | 5));
            if public.is_some() {
                assert!(
                    unsafe { dict_del_in_place(py, crate::ptr_from_bits(dictionary), name) }
                        .unwrap()
                );
            }
            if let Some(module) = existing {
                dec_ref_bits(py, module);
            }
            exceptions::exception_replace_field_bits(
                py,
                flag_error,
                ExceptionFieldSlot::Context,
                MoltObject::none().bits(),
            )
            .unwrap();
            for bits in [
                spec_base,
                spec_class,
                setter,
                loader_class,
                executor,
                creator,
                loader,
                name,
                spec,
                replacement,
                body_error,
                flag_error,
            ] {
                dec_ref_bits(py, bits);
            }
            assert!(!exception_pending(py));
        }
    });
}

#[test]
fn initial_finder_generation_preserves_public_customization_and_retires_owners() {
    use crate::builtins::exceptions::ExceptionValue;
    for mode in [
        "unchanged",
        "replaced",
        "none",
        "mutated-restored",
        "custom-hooks",
        "mutated-hooks",
    ] {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        crate::with_gil_entry_nopanic!(py, {
            let name = ExceptionValue::adopt(py, alloc_test_string_bits(py, "sys"));
            let sys = ExceptionValue::adopt(py, crate::molt_module_new(name.bits()));
            crate::builtins::module_table::publish_interpreter_sys_for_test(py, sys.bits());
            let read = |key| {
                let key = intern_runtime_static_name(py, key);
                ExceptionValue::adopt(
                    py,
                    crate::builtins::attributes::get_attr_name(py, sys.bits(), key).unwrap(),
                )
            };
            let meta = read(b"meta_path");
            let hooks = read(b"path_hooks");
            let initial = unsafe {
                crate::object::seq_access::snapshot(
                    py,
                    ptr_from_bits(meta.bits()),
                    "test initial finders",
                )
            }
            .unwrap();
            assert_eq!(initial.len(), 1, "one early runtime catalog finder");
            let finder = initial[0];
            let machinery = ExceptionValue::adopt(py, crate::molt_module_new(name.bits()));
            let path_finder = MoltObject::from_int(731).bits();
            loader_identity_setattr(py, machinery.bits(), b"PathFinder", path_finder);
            let defaults = ExceptionValue::adopt(
                py,
                MoltObject::from_ptr(alloc_list(py, &[MoltObject::from_int(419).bits()])).bits(),
            );
            loader_identity_setattr(
                py,
                machinery.bits(),
                b"_default_path_hooks",
                defaults.bits(),
            );
            let refs =
                |bits| unsafe { (*header_from_obj_ptr(ptr_from_bits(bits))).ref_count_snapshot() };
            match mode {
                "replaced" => {
                    let other =
                        ExceptionValue::adopt(py, MoltObject::from_ptr(alloc_list(py, &[])).bits());
                    loader_identity_setattr(py, sys.bits(), b"meta_path", other.bits());
                }
                "none" => {
                    loader_identity_setattr(py, sys.bits(), b"meta_path", MoltObject::none().bits())
                }
                "mutated-restored" => unsafe {
                    assert!(crate::object::list_mutation::clear(
                        py,
                        ptr_from_bits(meta.bits())
                    ));
                    assert!(crate::object::list_mutation::append(
                        py,
                        ptr_from_bits(meta.bits()),
                        finder
                    ));
                },
                "custom-hooks" => {
                    let custom = ExceptionValue::adopt(
                        py,
                        MoltObject::from_ptr(alloc_list(py, &[MoltObject::from_int(97).bits()]))
                            .bits(),
                    );
                    loader_identity_setattr(py, sys.bits(), b"path_hooks", custom.bits());
                }
                "mutated-hooks" => unsafe {
                    assert!(crate::object::list_mutation::append(
                        py,
                        ptr_from_bits(hooks.bits()),
                        MoltObject::from_int(1).bits()
                    ));
                    assert!(crate::object::list_mutation::clear(
                        py,
                        ptr_from_bits(hooks.bits())
                    ));
                },
                _ => (),
            }
            let before = (refs(meta.bits()), refs(hooks.bits()));
            importlib_finish_initial_defaults(py, unsafe {
                module_dict_bits(ptr_from_bits(machinery.bits()))
            })
            .expect("default generation disposition");
            assert!(!exception_pending(py));
            assert_eq!(
                (refs(meta.bits()), refs(hooks.bits())),
                (before.0 - 1, before.1 - 1),
                "terminal phase retires both temporary list owners"
            );
            {
                let current = read(b"meta_path");
                if mode == "none" {
                    assert!(obj_from_bits(current.bits()).is_none());
                } else {
                    let current = unsafe {
                        crate::object::seq_access::snapshot(
                            py,
                            ptr_from_bits(current.bits()),
                            "test final finders",
                        )
                    }
                    .unwrap();
                    let expected: &[u64] = match mode {
                        "replaced" => &[],
                        "mutated-restored" => &[finder],
                        _ => &[finder, path_finder],
                    };
                    assert_eq!(&*current, expected);
                }
                let current = read(b"path_hooks");
                let current = unsafe {
                    crate::object::seq_access::snapshot(
                        py,
                        ptr_from_bits(current.bits()),
                        "test final hooks",
                    )
                }
                .unwrap();
                let expected: &[u64] = match mode {
                    "custom-hooks" => &[MoltObject::from_int(97).bits()],
                    "mutated-hooks" => &[],
                    _ => &[MoltObject::from_int(419).bits()],
                };
                assert_eq!(&*current, expected);
            }
            importlib_finish_initial_defaults(py, unsafe {
                module_dict_bits(ptr_from_bits(machinery.bits()))
            })
            .expect("default generation disposition");
            assert_eq!(
                (refs(meta.bits()), refs(hooks.bits())),
                (before.0 - 1, before.1 - 1)
            );
        });
    }
}

#[test]
fn import_lock_reentry_failed_release_and_terminal_custody_share_one_owner() {
    let _transaction = crate::test_support::RuntimeTestTransaction::new();
    crate::with_gil_entry_nopanic!(py, {
        assert_eq!(molt_imp_lock_held(), MoltObject::from_bool(false).bits());
        assert_eq!(molt_imp_acquire_lock(), MoltObject::none().bits());
        let guard = ImportLockGuard::acquire(py).unwrap();
        assert_eq!(molt_imp_lock_held(), MoltObject::from_bool(true).bits());
        assert_eq!(molt_imp_release_lock(), MoltObject::none().bits());
        guard.finish(py).unwrap();
        assert_eq!(molt_imp_lock_held(), MoltObject::from_bool(false).bits());
        molt_imp_release_lock();
        assert_pending_exception_contains(py, "RuntimeError", &["not holding the import lock"]);
        importlib_retire_default_generation(py, runtime_state(py));
        molt_imp_acquire_lock();
        assert_pending_exception_contains(
            py,
            "ImportError",
            &["interpreter import state is retired"],
        );
        assert_eq!(molt_imp_lock_held(), MoltObject::from_bool(false).bits());
    });
}

#[cfg(not(target_arch = "wasm32"))]
static PLATFORM_FINALIZER_DISPOSITION: std::sync::atomic::AtomicUsize =
    std::sync::atomic::AtomicUsize::new(0);
#[cfg(not(target_arch = "wasm32"))]
extern "C" fn platform_retirement_finalizer(_self: u64) -> u64 {
    crate::with_gil_entry_nopanic!(py, {
        let platform = platform_state(py);
        let retired = platform.environment_mapping.load(Ordering::Acquire)
            == MoltObject::none().bits()
            && matches!(
                *platform.import_defaults.lock().unwrap(),
                DefaultFinderGeneration::Retired
            );
        PLATFORM_FINALIZER_DISPOSITION.store(if retired { 1 } else { 2 }, Ordering::Release);
        MoltObject::none().bits()
    })
}

#[test]
#[cfg(not(target_arch = "wasm32"))]
fn platform_terminal_custody_precedes_environment_value_finalizers() {
    crate::test_support::RuntimeTestTransaction::with_explicit_capabilities("env.write", |py| {
        PLATFORM_FINALIZER_DISPOSITION.store(0, Ordering::Release);
        molt_env_mapping_init();
        assert!(!exception_pending(py));
        let root = platform_state(py)
            .environment_mapping
            .load(Ordering::Acquire);
        let class =
            loader_identity_class(py, "TerminalEnvironmentOwner", builtin_classes(py).object);
        let function = crate::builtins::functions::alloc_runtime_function_obj(
            py,
            crate::builtins::functions::runtime_fn_addr(
                "platform_retirement_finalizer",
                platform_retirement_finalizer as *const (),
            ),
            1,
        );
        assert!(!function.is_null());
        let function = MoltObject::from_ptr(function).bits();
        // The class is already sealed: use normal mutation so its declared
        // finalizer metadata is published along with the namespace entry.
        loader_identity_setattr(py, class, b"__del__", function);
        dec_ref_bits(py, function);
        let value = unsafe { call_callable0(py, class) };
        assert!(!exception_pending(py));
        assert!(unsafe {
            crate::object::object_has_finalizer(py, obj_from_bits(value).as_ptr().unwrap())
        });
        let key = alloc_test_string_bits(py, "__MOLT_TERMINAL_CALLBACK__");
        assert!(unsafe { dict_set_in_place(py, ptr_from_bits(root), key, value) });
        dec_ref_bits(py, key);
        dec_ref_bits(py, value);
        assert_eq!(PLATFORM_FINALIZER_DISPOSITION.load(Ordering::Acquire), 0);
        assert!(platform_clear_runtime_state(py, runtime_state(py)));
        assert_eq!(
            PLATFORM_FINALIZER_DISPOSITION.load(Ordering::Acquire),
            1,
            "both terminal dispositions must precede the actual value finalizer"
        );
        assert!(!exception_pending(py));
        dec_ref_bits(py, class);
    });
}
