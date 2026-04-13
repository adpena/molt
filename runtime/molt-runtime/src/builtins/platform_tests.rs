use super::*;
use std::collections::BTreeMap;
use std::io::Write;

fn with_env_state<R>(entries: &[(&str, &str)], f: impl FnOnce() -> R) -> R {
    let _guard = crate::TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let original = {
        let mut env = env_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let original = env.clone();
        env.clear();
        for (key, value) in entries {
            env.insert((*key).to_string(), (*value).to_string());
        }
        original
    };
    let out = f();
    {
        let mut env = env_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        *env = original;
    }
    out
}

// Each test gets a fresh runtime by resetting the one-shot shutdown flag
// before re-initializing.  The `molt_runtime_reset_for_testing()` call is
// `#[cfg(test)]`-gated and safe here because `TEST_MUTEX` serializes access.
fn with_trusted_runtime<R>(f: impl FnOnce() -> R) -> R {
    let _guard = crate::TEST_MUTEX
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let prior = std::env::var("MOLT_TRUSTED").ok();
    unsafe {
        std::env::set_var("MOLT_TRUSTED", "1");
    }
    // Tear down any existing runtime.
    let _ = crate::state::runtime_state::molt_runtime_shutdown();
    // Reset the one-shot flags so `molt_runtime_init` can allocate a fresh
    // `RuntimeState`.  Without this, the `RUNTIME_SHUTDOWN_COMPLETE` flag
    // set by the shutdown above would permanently prevent re-initialization
    // for all subsequent tests in this process.
    crate::state::runtime_state::molt_runtime_reset_for_testing();
    let out = f();
    let _ = crate::state::runtime_state::molt_runtime_shutdown();
    // Reset again after the final shutdown so the next test (or any other
    // test in the process) can initialize the runtime from scratch.
    crate::state::runtime_state::molt_runtime_reset_for_testing();
    match prior {
        Some(value) => unsafe {
            std::env::set_var("MOLT_TRUSTED", value);
        },
        None => unsafe {
            std::env::remove_var("MOLT_TRUSTED");
        },
    }
    out
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

fn alloc_test_string_bits(_py: &PyToken<'_>, value: &str) -> u64 {
    let ptr = alloc_string(_py, value.as_bytes());
    assert!(!ptr.is_null(), "alloc string failed for {value:?}");
    MoltObject::from_ptr(ptr).bits()
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

fn call_extension_exec_boundary(
    _py: &PyToken<'_>,
    namespace_bits: u64,
    module_name: &str,
    path: &str,
) -> u64 {
    let module_bits = alloc_test_string_bits(_py, module_name);
    let path_bits = alloc_test_string_bits(_py, path);
    let out = molt_importlib_exec_extension(namespace_bits, module_bits, path_bits);
    dec_ref_bits(_py, module_bits);
    dec_ref_bits(_py, path_bits);
    out
}

fn extension_spec_bits_for_tests(_py: &PyToken<'_>, module_name: &str, origin: &str) -> u64 {
    let spec_bits = unsafe { call_callable0(_py, builtin_classes(_py).object) };
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
fn sys_bootstrap_state_includes_pythonpath_module_roots_and_pwd() {
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
            ("MOLT_DEV_TRUSTED", "1"),
            ("PWD", "/tmp/molt_pwd"),
        ],
        || {
            let state = sys_bootstrap_state_from_module_file(Some(bootstrap_module_file()));
            let path_sep = if sys_platform_str().starts_with("win") {
                '\\'
            } else {
                '/'
            };
            let expected_alpha = bootstrap_resolve_path_entry("alpha", "/tmp/molt_pwd", path_sep);
            let expected_beta = bootstrap_resolve_path_entry("beta", "/tmp/molt_pwd", path_sep);
            let expected_gamma = bootstrap_resolve_path_entry("gamma", "/tmp/molt_pwd", path_sep);
            let expected_delta = bootstrap_resolve_path_entry("delta", "/tmp/molt_pwd", path_sep);
            assert_eq!(
                state.pythonpath_entries,
                vec![expected_alpha.clone(), expected_beta.clone()]
            );
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
                    expected_alpha,
                    expected_beta,
                    expected_stdlib_root(),
                    expected_gamma,
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
            ("MOLT_DEV_TRUSTED", "0"),
            ("PWD", "/tmp/molt_pwd"),
        ],
        || {
            let state = sys_bootstrap_state_from_module_file(Some(bootstrap_module_file()));
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
            ("MOLT_DEV_TRUSTED", "1"),
            ("PWD", "/tmp/molt_pwd"),
        ],
        || {
            let state =
                sys_bootstrap_state_from_module_file(Some(bootstrap_stdlib_submodule_file()));
            assert_eq!(state.stdlib_root, Some(expected_stdlib_root()));
            assert!(state.path.iter().any(|entry| entry == &expected_stdlib_root()));
        },
    );
}

#[test]
fn sys_bootstrap_state_falls_back_to_current_dir_when_pwd_missing() {
    with_env_state(
        &[
            ("PYTHONPATH", ""),
            ("MOLT_MODULE_ROOTS", ""),
            ("MOLT_DEV_TRUSTED", "1"),
            ("PWD", ""),
        ],
        || {
            let state = sys_bootstrap_state_from_module_file(Some(bootstrap_module_file()));
            assert!(state.include_cwd);
            assert_eq!(state.pwd, resolve_bootstrap_pwd(""));
            assert!(state.path.iter().any(|entry| entry.is_empty()));
        },
    );
}

#[test]
fn sys_bootstrap_state_includes_virtual_env_site_packages_when_present() {
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
            ("MOLT_DEV_TRUSTED", "1"),
            ("PWD", "/tmp/molt_pwd"),
        ],
        || {
            let state = sys_bootstrap_state_from_module_file(Some(bootstrap_module_file()));
            assert_eq!(state.virtual_env_raw, venv_root_text);
            assert!(
                state
                    .venv_site_packages_entries
                    .iter()
                    .any(|entry| entry == &site_packages_text)
            );
            assert!(state.path.iter().any(|entry| entry == &site_packages_text));
        },
    );
    std::fs::remove_dir_all(&tmp).expect("cleanup virtualenv temp dirs");
}

#[test]
fn runpy_resolve_path_uses_bootstrap_pwd_for_relative_paths() {
    with_env_state(
        &[
            ("PYTHONPATH", ""),
            ("MOLT_MODULE_ROOTS", ""),
            ("MOLT_DEV_TRUSTED", "1"),
            ("PWD", "/tmp/bootstrap_pwd"),
        ],
        || {
            let resolved =
                bootstrap_resolve_abspath("pkg/../mod.py", Some(bootstrap_module_file()));
            assert_eq!(resolved, "/tmp/bootstrap_pwd/mod.py");
        },
    );
}

#[test]
fn importlib_source_loader_resolution_marks_packages() {
    let package = source_loader_resolution("demo.pkg", "/tmp/demo/pkg/__init__.py", false);
    assert!(package.is_package);
    assert_eq!(package.module_package, "demo.pkg");
    assert_eq!(package.package_root, Some("/tmp/demo/pkg".to_string()));

    let module = source_loader_resolution("demo.pkg.mod", "/tmp/demo/pkg/mod.py", false);
    assert!(!module.is_package);
    assert_eq!(module.module_package, "demo.pkg");
    assert_eq!(module.package_root, None);
}

#[test]
fn importlib_source_exec_payload_reads_source_and_resolution() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!(
        "molt_importlib_source_exec_payload_{}_{}",
        std::process::id(),
        stamp
    ));
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let module_path = tmp.join("demo.py");
    std::fs::write(&module_path, "value = 42\n").expect("write module source");

    let payload = importlib_source_exec_payload("demo", &module_path.to_string_lossy(), false)
        .expect("build source exec payload");
    assert!(!payload.is_package);
    assert_eq!(payload.module_package, "");
    assert_eq!(payload.package_root, None);
    let text = String::from_utf8(payload.source.clone()).expect("decode source text");
    assert!(text.contains("value = 42"));

    std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
}

#[test]
fn importlib_cache_from_source_matches_cpython_layout() {
    assert_eq!(
        importlib_cache_from_source("/tmp/pkg/mod.py"),
        "/tmp/pkg/__pycache__/mod.pyc"
    );
    assert_eq!(importlib_cache_from_source("/tmp/pkg/mod"), "/tmp/pkg/modc");
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
    assert_eq!(pkg.cached, Some(importlib_cache_from_source(&pkg_origin)));
    assert!(pkg.has_location);
    assert_eq!(pkg.loader_kind, "source");

    let module = importlib_find_in_path("moddemo", &search_paths, false).expect("module spec");
    assert!(!module.is_package);
    let module_origin = module.origin.clone().expect("module origin");
    assert!(module_origin.ends_with("moddemo.py"));
    assert_eq!(module.submodule_search_locations, None);
    assert_eq!(
        module.cached,
        Some(importlib_cache_from_source(&module_origin))
    );
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
#[ignore = "calls molt_runtime_shutdown() which sets RUNTIME_SHUTDOWN_COMPLETE and prevents runtime re-init in the same process; run in isolation with `cargo test -- extension_spec_boundary_rejects_missing_manifest_sidecar --ignored`"]
fn extension_spec_boundary_rejects_missing_manifest_sidecar() {
    with_trusted_runtime(|| {
        clear_extension_metadata_validation_cache();
        let tmp = extension_boundary_temp_dir("molt_extension_spec_missing_manifest");
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let module_name = format!("ext_spec_missing_{}", std::process::id());
        let filename = extension_boundary_module_filename(&module_name);
        let extension_path = tmp.join(&filename);
        std::fs::write(&extension_path, b"spec-boundary-extension")
            .expect("write extension placeholder");
        let search_paths = vec![tmp.to_string_lossy().into_owned()];

        crate::with_gil_entry!(_py, {
            let out =
                importlib_find_spec_payload(_py, &module_name, &search_paths, None, 1, 0, false);
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
#[ignore = "calls molt_runtime_shutdown() which sets RUNTIME_SHUTDOWN_COMPLETE and prevents runtime re-init in the same process; run in isolation with `cargo test -- extension_spec_boundary_rejects_invalid_manifest_payload --ignored`"]
fn extension_spec_boundary_rejects_invalid_manifest_payload() {
    with_trusted_runtime(|| {
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

        crate::with_gil_entry!(_py, {
            let out =
                importlib_find_spec_payload(_py, &module_name, &search_paths, None, 1, 0, false);
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
#[ignore = "calls molt_runtime_shutdown() which sets RUNTIME_SHUTDOWN_COMPLETE and prevents runtime re-init in the same process; run in isolation with `cargo test -- extension_spec_boundary_accepts_valid_manifest --ignored`"]
fn extension_spec_boundary_accepts_valid_manifest() {
    with_trusted_runtime(|| {
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

        crate::with_gil_entry!(_py, {
            let payload =
                importlib_find_spec_payload(_py, &module_name, &search_paths, None, 1, 0, false)
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
#[ignore = "calls molt_runtime_shutdown() which sets RUNTIME_SHUTDOWN_COMPLETE and prevents runtime re-init in the same process; run in isolation with `cargo test -- extension_spec_boundary_rejects_manifest_module_mismatch --ignored`"]
fn extension_spec_boundary_rejects_manifest_module_mismatch() {
    with_trusted_runtime(|| {
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

        crate::with_gil_entry!(_py, {
            let out =
                importlib_find_spec_payload(_py, &module_name, &search_paths, None, 1, 0, false);
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
#[ignore = "calls molt_runtime_shutdown() which sets RUNTIME_SHUTDOWN_COMPLETE and prevents runtime re-init in the same process; run in isolation with `cargo test -- extension_spec_boundary_revalidates_cache_after_artifact_mutation --ignored`"]
fn extension_spec_boundary_revalidates_cache_after_artifact_mutation() {
    with_trusted_runtime(|| {
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

        crate::with_gil_entry!(_py, {
            let payload =
                importlib_find_spec_payload(_py, &module_name, &search_paths, None, 1, 0, false)
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

        crate::with_gil_entry!(_py, {
            let out =
                importlib_find_spec_payload(_py, &module_name, &search_paths, None, 1, 0, false);
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
#[ignore = "calls molt_runtime_shutdown() which sets RUNTIME_SHUTDOWN_COMPLETE and prevents runtime re-init in the same process; run in isolation with `cargo test -- extension_spec_object_boundary_enforces_missing_and_valid_manifest --ignored`"]
fn extension_spec_object_boundary_enforces_missing_and_valid_manifest() {
    with_trusted_runtime(|| {
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

            crate::with_gil_entry!(_py, {
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

            crate::with_gil_entry!(_py, {
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
#[ignore = "calls molt_runtime_shutdown() which sets RUNTIME_SHUTDOWN_COMPLETE and prevents runtime re-init in the same process; run in isolation with `cargo test -- extension_loader_boundary_rejects_missing_manifest_sidecar --ignored`"]
fn extension_loader_boundary_rejects_missing_manifest_sidecar() {
    with_trusted_runtime(|| {
        clear_extension_metadata_validation_cache();
        let tmp = extension_boundary_temp_dir("molt_extension_loader_missing_manifest");
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let extension_path = tmp.join(extension_boundary_filename());
        std::fs::write(&extension_path, b"loader-boundary-extension")
            .expect("write extension placeholder");
        let module_name = "demo.extension.loader.missing";
        let extension_path_text = extension_path.to_string_lossy().into_owned();

        crate::with_gil_entry!(_py, {
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
#[ignore = "calls molt_runtime_shutdown() which sets RUNTIME_SHUTDOWN_COMPLETE and prevents runtime re-init in the same process; run in isolation with `cargo test -- extension_loader_boundary_rejects_invalid_manifest_payload --ignored`"]
fn extension_loader_boundary_rejects_invalid_manifest_payload() {
    with_trusted_runtime(|| {
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

        crate::with_gil_entry!(_py, {
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
#[ignore = "calls molt_runtime_shutdown() which sets RUNTIME_SHUTDOWN_COMPLETE and prevents runtime re-init in the same process; run in isolation with `cargo test -- extension_exec_boundary_rejects_missing_manifest_sidecar --ignored`"]
fn extension_exec_boundary_rejects_missing_manifest_sidecar() {
    with_trusted_runtime(|| {
        clear_extension_metadata_validation_cache();
        let tmp = extension_boundary_temp_dir("molt_extension_exec_missing_manifest");
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let extension_path = tmp.join(extension_boundary_filename());
        std::fs::write(&extension_path, b"exec-boundary-extension")
            .expect("write extension placeholder");
        let module_name = "demo.extension.exec.missing";
        let extension_path_text = extension_path.to_string_lossy().into_owned();

        crate::with_gil_entry!(_py, {
            let namespace_ptr = alloc_dict_with_pairs(_py, &[]);
            assert!(!namespace_ptr.is_null(), "alloc namespace dict");
            let namespace_bits = MoltObject::from_ptr(namespace_ptr).bits();
            let _ = call_extension_exec_boundary(
                _py,
                namespace_bits,
                module_name,
                &extension_path_text,
            );
            dec_ref_bits(_py, namespace_bits);
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
#[ignore = "calls molt_runtime_shutdown() which sets RUNTIME_SHUTDOWN_COMPLETE and prevents runtime re-init in the same process; run in isolation with `cargo test -- extension_exec_boundary_rejects_invalid_manifest_metadata --ignored`"]
fn extension_exec_boundary_rejects_invalid_manifest_metadata() {
    with_trusted_runtime(|| {
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

        crate::with_gil_entry!(_py, {
            let namespace_ptr = alloc_dict_with_pairs(_py, &[]);
            assert!(!namespace_ptr.is_null(), "alloc namespace dict");
            let namespace_bits = MoltObject::from_ptr(namespace_ptr).bits();
            let _ = call_extension_exec_boundary(
                _py,
                namespace_bits,
                module_name,
                &extension_path_text,
            );
            dec_ref_bits(_py, namespace_bits);
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
#[ignore = "calls molt_runtime_shutdown() which sets RUNTIME_SHUTDOWN_COMPLETE and prevents runtime re-init in the same process; run in isolation with `cargo test -- extension_loader_boundary_rejects_manifest_module_mismatch --ignored`"]
fn extension_loader_boundary_rejects_manifest_module_mismatch() {
    with_trusted_runtime(|| {
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

        crate::with_gil_entry!(_py, {
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
#[ignore = "calls molt_runtime_shutdown() which sets RUNTIME_SHUTDOWN_COMPLETE and prevents runtime re-init in the same process; run in isolation with `cargo test -- extension_exec_boundary_rejects_manifest_module_mismatch --ignored`"]
fn extension_exec_boundary_rejects_manifest_module_mismatch() {
    with_trusted_runtime(|| {
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

        crate::with_gil_entry!(_py, {
            let namespace_ptr = alloc_dict_with_pairs(_py, &[]);
            assert!(!namespace_ptr.is_null(), "alloc namespace dict");
            let namespace_bits = MoltObject::from_ptr(namespace_ptr).bits();
            let _ = call_extension_exec_boundary(
                _py,
                namespace_bits,
                module_name,
                &extension_path_text,
            );
            dec_ref_bits(_py, namespace_bits);
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
#[ignore = "calls molt_runtime_shutdown() which sets RUNTIME_SHUTDOWN_COMPLETE and prevents runtime re-init in the same process; run in isolation with `cargo test -- extension_loader_boundary_revalidates_cache_after_artifact_mutation --ignored`"]
fn extension_loader_boundary_revalidates_cache_after_artifact_mutation() {
    with_trusted_runtime(|| {
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

        crate::with_gil_entry!(_py, {
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

        crate::with_gil_entry!(_py, {
            let _ = call_extension_loader_boundary(_py, module_name, &extension_path_text);
            assert_pending_exception_contains(_py, "ImportError", &["extension checksum mismatch"]);
        });

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    });
}

#[test]
#[ignore = "calls molt_runtime_shutdown() which sets RUNTIME_SHUTDOWN_COMPLETE and prevents runtime re-init in the same process; run in isolation with `cargo test -- extension_loader_boundary_records_cache_hits_and_misses --ignored`"]
fn extension_loader_boundary_records_cache_hits_and_misses() {
    with_trusted_runtime(|| {
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

        crate::with_gil_entry!(_py, {
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
fn importlib_stabilize_module_state_ignores_missing_dunder_path_on_plain_module() {
    with_trusted_runtime(|| {
        crate::with_gil_entry!(_py, {
            let name_bits = alloc_test_string_bits(_py, "math");
            let module_bits = crate::molt_module_new(name_bits);
            assert!(
                !exception_pending(_py),
                "failed to allocate plain module: {:?}",
                pending_exception_kind_and_message(_py)
            );

            let origin_bits = alloc_test_string_bits(_py, "/repo/src/molt/stdlib/math.py");
            let package_bits = alloc_test_string_bits(_py, "");
            let result_bits = molt_importlib_stabilize_module_state(
                module_bits,
                MoltObject::none().bits(),
                origin_bits,
                MoltObject::from_bool(false).bits(),
                package_bits,
                MoltObject::none().bits(),
            );
            assert!(
                !exception_pending(_py),
                "unexpected stabilize exception: {:?}",
                pending_exception_kind_and_message(_py)
            );
            assert!(obj_from_bits(result_bits).is_none());

            let path_name_bits =
                attr_name_bits_from_bytes(_py, b"__path__").expect("intern __path__");
            let path_bits = getattr_optional_bits(_py, module_bits, path_name_bits)
                .expect("raw __path__ lookup on plain module");
            assert!(path_bits.is_none(), "plain module unexpectedly retained __path__");

            dec_ref_bits(_py, path_name_bits);
            dec_ref_bits(_py, package_bits);
            dec_ref_bits(_py, origin_bits);
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, name_bits);
        });
    });
}

#[test]
fn importlib_stabilize_module_state_clears_internal_dunder_path_placeholder() {
    with_trusted_runtime(|| {
        crate::with_gil_entry!(_py, {
            let name_bits = alloc_test_string_bits(_py, "math");
            let module_bits = crate::molt_module_new(name_bits);
            assert!(
                !exception_pending(_py),
                "failed to allocate plain module: {:?}",
                pending_exception_kind_and_message(_py)
            );
            let module_ptr = obj_from_bits(module_bits)
                .as_ptr()
                .expect("plain module pointer");
            let module_dict_bits = unsafe { module_dict_bits(module_ptr) };
            let module_dict_ptr = obj_from_bits(module_dict_bits)
                .as_ptr()
                .expect("plain module dict pointer");
            let path_name_bits =
                attr_name_bits_from_bytes(_py, b"__path__").expect("intern __path__");
            let placeholder_bits = unsafe { call_callable0(_py, builtin_classes(_py).object) };
            assert!(
                !exception_pending(_py),
                "failed to allocate placeholder object: {:?}",
                pending_exception_kind_and_message(_py)
            );
            unsafe {
                dict_set_in_place(_py, module_dict_ptr, path_name_bits, placeholder_bits);
            }
            assert!(
                !exception_pending(_py),
                "failed to seed __path__ placeholder: {:?}",
                pending_exception_kind_and_message(_py)
            );

            let origin_bits = alloc_test_string_bits(_py, "/repo/src/molt/stdlib/math.py");
            let package_bits = alloc_test_string_bits(_py, "");
            let result_bits = molt_importlib_stabilize_module_state(
                module_bits,
                MoltObject::none().bits(),
                origin_bits,
                MoltObject::from_bool(false).bits(),
                package_bits,
                MoltObject::none().bits(),
            );
            assert!(
                !exception_pending(_py),
                "unexpected stabilize exception with placeholder __path__: {:?}",
                pending_exception_kind_and_message(_py)
            );
            assert!(obj_from_bits(result_bits).is_none());

            let path_bits = getattr_optional_bits(_py, module_bits, path_name_bits)
                .expect("raw __path__ lookup after stabilize");
            assert!(path_bits.is_none(), "internal __path__ placeholder was not cleared");

            dec_ref_bits(_py, placeholder_bits);
            dec_ref_bits(_py, path_name_bits);
            dec_ref_bits(_py, package_bits);
            dec_ref_bits(_py, origin_bits);
            dec_ref_bits(_py, module_bits);
            dec_ref_bits(_py, name_bits);
        });
    });
}

#[test]
#[cfg_attr(miri, ignore)]
fn importlib_sha256_path_supports_zip_archive_members() {
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
    crate::with_gil_entry!(_py, {
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
            ("MOLT_DEV_TRUSTED", "1"),
            ("PWD", "/tmp/bootstrap_pwd"),
        ],
        || {
            let resolved =
                importlib_search_paths(&["src".to_string()], Some(bootstrap_module_file()));
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
    let resolved =
        importlib_namespace_paths("nsdemo", &search_paths, Some(bootstrap_module_file()));
    assert!(resolved.iter().any(|entry| entry == &ns_one_text));
    assert!(resolved.iter().any(|entry| entry == &ns_two_text));
    std::fs::remove_dir_all(&tmp).expect("cleanup temp dirs");
}

#[test]
#[cfg_attr(miri, ignore)]
fn importlib_namespace_paths_finds_zip_namespace_dirs() {
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
    let resolved =
        importlib_namespace_paths("nszip.pkg", &[archive_text], Some(bootstrap_module_file()));
    assert!(resolved.iter().any(|entry| entry == &expected));

    std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
}

#[test]
fn importlib_metadata_dist_paths_finds_dist_info_dirs() {
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
    let resolved = importlib_metadata_dist_paths(
        &[tmp.to_string_lossy().into_owned()],
        Some(bootstrap_module_file()),
    );
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
fn importlib_resources_path_payload_reports_entries_and_init_marker() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!(
        "molt_importlib_resources_payload_{}_{}",
        std::process::id(),
        stamp
    ));
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    std::fs::write(tmp.join("__init__.py"), "x = 1\n").expect("write __init__.py");
    std::fs::write(tmp.join("data.txt"), "payload\n").expect("write data.txt");

    let payload = importlib_resources_path_payload(&tmp.to_string_lossy());
    assert!(payload.exists);
    assert!(payload.is_dir);
    assert!(!payload.is_file);
    assert!(payload.has_init_py);
    assert!(!payload.is_archive_member);
    assert!(payload.entries.iter().any(|entry| entry == "__init__.py"));
    assert!(payload.entries.iter().any(|entry| entry == "data.txt"));

    std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
}

#[test]
#[cfg_attr(miri, ignore)]
fn importlib_resources_zip_payload_reports_entries_and_init_marker() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!(
        "molt_importlib_resources_zip_payload_{}_{}",
        std::process::id(),
        stamp
    ));
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let archive = tmp.join("resources.zip");
    let file = std::fs::File::create(&archive).expect("create zip file");
    let mut writer = zip::ZipWriter::new(file);
    let options: zip::write::SimpleFileOptions = zip::write::FileOptions::default();
    writer
        .start_file("pkg/__init__.py", options)
        .expect("start __init__.py");
    writer
        .write_all(b"x = 1\n")
        .expect("write __init__.py in zip");
    writer
        .start_file("pkg/data.txt", options)
        .expect("start data.txt");
    writer
        .write_all(b"payload\n")
        .expect("write data.txt in zip");
    writer.finish().expect("finish zip archive");

    let archive_text = archive.to_string_lossy().into_owned();
    let package_root = format!("{archive_text}/pkg");
    let package_payload = importlib_resources_path_payload(&package_root);
    assert!(package_payload.exists);
    assert!(package_payload.is_dir);
    assert!(!package_payload.is_file);
    assert!(package_payload.has_init_py);
    assert!(package_payload.is_archive_member);
    assert!(
        package_payload
            .entries
            .iter()
            .any(|entry| entry == "__init__.py")
    );
    assert!(
        package_payload
            .entries
            .iter()
            .any(|entry| entry == "data.txt")
    );

    let file_payload = importlib_resources_path_payload(&format!("{package_root}/data.txt"));
    assert!(file_payload.exists);
    assert!(file_payload.is_file);
    assert!(!file_payload.is_dir);
    assert!(!file_payload.has_init_py);
    assert!(file_payload.is_archive_member);

    let package_meta = importlib_resources_package_payload(
        "pkg",
        std::slice::from_ref(&archive_text),
        Some(bootstrap_module_file()),
    );
    assert!(package_meta.has_regular_package);
    assert!(
        package_meta
            .roots
            .iter()
            .any(|entry| entry == &package_root)
    );
    assert!(
        package_meta
            .init_file
            .as_deref()
            .is_some_and(|entry| entry.ends_with("resources.zip/pkg/__init__.py"))
    );

    std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
}

#[test]
#[cfg_attr(miri, ignore)]
fn importlib_resources_whl_payload_reports_archive_member_flag() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!(
        "molt_importlib_resources_whl_payload_{}_{}",
        std::process::id(),
        stamp
    ));
    std::fs::create_dir_all(&tmp).expect("create temp dir");
    let archive = tmp.join("resources.whl");
    let file = std::fs::File::create(&archive).expect("create whl file");
    let mut writer = zip::ZipWriter::new(file);
    let options: zip::write::SimpleFileOptions = zip::write::FileOptions::default();
    writer
        .start_file("pkg/data.txt", options)
        .expect("start data.txt");
    writer
        .write_all(b"payload\n")
        .expect("write data.txt in whl");
    writer.finish().expect("finish whl archive");

    let archive_text = archive.to_string_lossy().into_owned();
    let file_payload = importlib_resources_path_payload(&format!("{archive_text}/pkg/data.txt"));
    assert!(file_payload.exists);
    assert!(file_payload.is_file);
    assert!(file_payload.is_archive_member);

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

#[test]
fn importlib_metadata_record_payload_parses_rows() {
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let tmp = std::env::temp_dir().join(format!(
        "molt_importlib_metadata_record_payload_{}_{}",
        std::process::id(),
        stamp
    ));
    let dist = tmp.join("demo_record-1.0.dist-info");
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

    std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
}

#[test]
fn importlib_metadata_packages_distributions_payload_aggregates_top_level() {
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

    let payload = importlib_metadata_packages_distributions_payload(
        &[tmp.to_string_lossy().into_owned()],
        Some(bootstrap_module_file()),
    );
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
            ("MOLT_DEV_TRUSTED", "1"),
            ("PWD", "/tmp/bootstrap_pwd"),
        ],
        || {
            let payload =
                importlib_bootstrap_payload(&["src".to_string()], Some(bootstrap_module_file()));
            let path_sep = if sys_platform_str().starts_with("win") {
                '\\'
            } else {
                '/'
            };
            let expected_alpha =
                bootstrap_resolve_path_entry("alpha", "/tmp/bootstrap_pwd", path_sep);
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
            assert_eq!(payload.pythonpath_entries, vec![expected_alpha]);
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
    let payload = importlib_metadata_entry_points_payload(
        &[tmp.to_string_lossy().into_owned()],
        Some(bootstrap_module_file()),
    );
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
    let group_filtered = importlib_metadata_entry_points_select_payload(
        &search_paths,
        Some(bootstrap_module_file()),
        Some("console_scripts"),
        None,
    );
    assert_eq!(group_filtered.len(), 2);
    assert!(
        group_filtered
            .iter()
            .all(|(_, _, group)| group == "console_scripts")
    );
    let name_filtered = importlib_metadata_entry_points_select_payload(
        &search_paths,
        Some(bootstrap_module_file()),
        Some("console_scripts"),
        Some("beta"),
    );
    assert_eq!(name_filtered.len(), 1);
    assert_eq!(name_filtered[0].0, "beta");
    std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
}

#[test]
fn importlib_metadata_entry_points_filter_payload_filters_by_value() {
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
    let filtered = importlib_metadata_entry_points_filter_payload(
        &search_paths,
        Some(bootstrap_module_file()),
        Some("demo.group"),
        None,
        Some("demo:beta"),
    );
    assert_eq!(filtered.len(), 1);
    assert_eq!(filtered[0].0, "beta");
    assert_eq!(filtered[0].1, "demo:beta");
    std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
}

#[test]
fn importlib_metadata_distributions_payload_aggregates_dist_payloads() {
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

    let payloads = importlib_metadata_distributions_payload(
        &[tmp.to_string_lossy().into_owned()],
        Some(bootstrap_module_file()),
    );
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
