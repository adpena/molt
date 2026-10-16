use super::*;
use std::sync::Arc;

const SHARED_STDLIB_CACHE_ENV_KEYS: &[&str] = &[
    "MOLT_STDLIB_OBJ",
    "MOLT_STDLIB_CACHE_ENTRY",
    "MOLT_STDLIB_CACHE_KEY",
    "MOLT_STDLIB_CACHE_MANIFEST",
];

#[test]
#[cfg(all(feature = "native-backend", any(unix, windows)))]
fn daemon_non_unicode_stdlib_request_cannot_bypass_admission_on_any_native_transport() {
    let _env = TestEnvGuard::clear(SHARED_STDLIB_CACHE_ENV_KEYS);
    #[cfg(unix)]
    let invalid = {
        use std::os::unix::ffi::OsStringExt;
        std::ffi::OsString::from_vec(vec![0xff])
    };
    #[cfg(windows)]
    let invalid = {
        use std::os::windows::ffi::OsStringExt;
        std::ffi::OsString::from_wide(&[0xd800])
    };
    unsafe {
        std::env::set_var("MOLT_STDLIB_OBJ", invalid);
    }
    let root = std::env::temp_dir().join(format!(
        "molt-daemon-nonunicode-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("create fixture");
    for kind in ["object", "archive"] {
        for probe in [false, true] {
            for warm in [false, true] {
                let cache = DaemonCache::new(None);
                if warm {
                    cache.insert(
                        format!("artifact-v1:{kind}:same"),
                        Arc::from(b"cached".as_slice()),
                    );
                }
                let output = root.join(format!("{kind}-{probe}-{warm}"));
                let value = serde_json::json!({
                    "id":"invalid", "is_wasm":false, "native_output_kind":kind,
                    "output":output.to_string_lossy(), "cache_key":"same", "probe_cache_only":probe
                });
                let result = compile_single_job(
                    DaemonJobRequest::from_json_value(&value, "job").expect("parse job"),
                    &cache,
                );
                assert!(!result.ok && !result.cached && !result.needs_ir && !result.output_written);
                assert!(
                    result
                        .message
                        .expect("encoding diagnostic")
                        .contains("Unicode")
                );
                assert!(!output.exists());
            }
        }
    }
    // The native stdlib request does not reach a WASM job.
    #[cfg(feature = "wasm-backend")]
    {
        let cache = DaemonCache::new(None);
        cache.insert(
            "artifact-v1:wasm:same".into(),
            Arc::from(b"wasm".as_slice()),
        );
        let output = root.join("wasm-output");
        let value = serde_json::json!({
            "id":"wasm", "is_wasm":true, "output":output.to_string_lossy(),
            "cache_key":"same", "probe_cache_only":true
        });
        let result = compile_single_job(
            DaemonJobRequest::from_json_value(&value, "job").expect("parse wasm"),
            &cache,
        );
        assert!(result.ok && result.cached && result.output_written);
    }
    std::fs::remove_dir_all(root).expect("remove fixture");
}

#[test]
#[cfg(feature = "native-backend")]
fn daemon_native_object_contract_is_checked_before_cached_or_probe_admission() {
    let _env = TestEnvGuard::clear(SHARED_STDLIB_CACHE_ENV_KEYS);
    let root = std::env::temp_dir().join(format!(
        "molt-daemon-output-admission-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("create fixture");
    let stdlib = root.join("stdlib.a");
    let mut object = object::write::Object::new(
        object::BinaryFormat::Elf,
        object::Architecture::X86_64,
        object::Endianness::Little,
    );
    object.add_section(Vec::new(), b".text".to_vec(), object::SectionKind::Text);
    crate::backend_process::write_native_archive_bytes(
        &stdlib,
        &object.write().expect("fixture object"),
    )
    .expect("fixture archive");
    write_shared_stdlib_cache_sidecars(
        &stdlib,
        0,
        Some("stdlib-key"),
        Some("manifest"),
        "partition",
    )
    .expect("matching stdlib generation");
    unsafe {
        std::env::set_var("MOLT_STDLIB_OBJ", &stdlib);
        std::env::set_var("MOLT_STDLIB_CACHE_KEY", "stdlib-key");
        std::env::set_var("MOLT_STDLIB_CACHE_MANIFEST", "manifest");
    }
    for probe in [false, true] {
        for warm in [false, true] {
            let cache = DaemonCache::new(None);
            if warm {
                cache.insert(
                    "artifact-v1:object:same".into(),
                    Arc::from(b"cached object".as_slice()),
                );
            }
            let output = root.join(format!("object-{probe}-{warm}.o"));
            let value = serde_json::json!({
                "id": "invalid-object", "is_wasm": false, "native_output_kind": "object",
                "output": output.to_string_lossy(), "cache_key": "same", "probe_cache_only": probe
            });
            let job = DaemonJobRequest::from_json_value(&value, "job").expect("parse job");
            let result = compile_single_job(job, &cache);
            assert!(!result.ok && !result.cached && !result.needs_ir && !result.output_written);
            assert!(
                result
                    .message
                    .expect("contract error")
                    .contains("complete graph")
            );
            assert!(!output.exists(), "invalid request must not replay output");
        }
    }
    // Native-only extraction state must not restrict a WASM cache request.
    #[cfg(feature = "wasm-backend")]
    {
        let cache = DaemonCache::new(None);
        cache.insert(
            "artifact-v1:wasm:same".into(),
            Arc::from(b"wasm cache".as_slice()),
        );
        let output = root.join("wasm-output.wasm");
        let value = serde_json::json!({
            "id": "wasm", "is_wasm": true, "output": output.to_string_lossy(),
            "cache_key": "same", "probe_cache_only": true
        });
        let result = compile_single_job(
            DaemonJobRequest::from_json_value(&value, "job").expect("parse wasm job"),
            &cache,
        );
        assert!(result.ok && result.cached && result.output_written);
        assert_eq!(
            std::fs::read(output).expect("wasm cache replay"),
            b"wasm cache"
        );
    }
    std::fs::remove_dir_all(root).expect("remove fixture");
}

#[test]
#[cfg(feature = "native-backend")]
fn daemon_cache_artifact_kinds_cannot_alias_even_for_equal_caller_keys() {
    let _env = TestEnvGuard::clear(SHARED_STDLIB_CACHE_ENV_KEYS);
    let cache = DaemonCache::new(None);
    let root = std::env::temp_dir().join(format!(
        "molt-daemon-artifact-kinds-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("create test directory");
    for (kind, expected) in [
        ("object", b"object".as_slice()),
        ("archive", b"archive".as_slice()),
    ] {
        let output = root.join(kind);
        let request = serde_json::json!({"id":kind, "is_wasm":false, "native_output_kind":kind, "output":output.to_string_lossy(), "cache_key":"same", "function_cache_key":"same-functions", "probe_cache_only":true});
        let job = DaemonJobRequest::from_json_value(&request, "job").expect("parse job");
        let miss = compile_single_job(job, &cache);
        assert!(
            miss.needs_ir,
            "other artifact kinds cannot satisfy this cache probe"
        );
        cache.insert(
            format!("artifact-v1:{kind}:same-functions"),
            Arc::from(expected),
        );
        let job = DaemonJobRequest::from_json_value(&request, "job").expect("parse replay job");
        let hit = compile_single_job(job, &cache);
        assert!(hit.cached && hit.output_written);
        assert_eq!(std::fs::read(&output).expect("read replay"), expected);
    }
    std::fs::remove_dir_all(root).expect("clean test directory");
}

#[test]
fn daemon_cache_get_bytes_updates_lru_without_cloning() {
    let mut cache = DaemonLru::new(None);
    cache.insert("module".to_string(), Arc::from(vec![1, 2, 3, 4]));

    let bytes = cache.get_bytes("module").expect("cache hit");
    assert_eq!(bytes.as_ref(), &[1, 2, 3, 4]);

    let entry = cache.entries.get("module").expect("entry retained");
    assert_eq!(entry.bytes.as_ref(), &[1, 2, 3, 4]);
    assert_eq!(entry.stamp, cache.clock);
}

#[test]
fn daemon_cache_can_share_bytes_across_keys() {
    let mut cache = DaemonLru::new(None);
    let shared = Arc::<[u8]>::from(vec![9, 8, 7, 6]);
    cache.insert("module".to_string(), Arc::clone(&shared));
    cache.insert("function".to_string(), shared);

    let module = cache.entries.get("module").expect("module entry");
    let function = cache.entries.get("function").expect("function entry");
    assert!(Arc::ptr_eq(&module.bytes, &function.bytes));
}

#[test]
fn daemon_default_cache_limit_scales_with_host_memory() {
    assert_eq!(
        default_daemon_cache_bytes_from_physical_mem_bytes(Some(8 * GIB)),
        128 * MIB
    );
    assert_eq!(
        default_daemon_cache_bytes_from_physical_mem_bytes(Some(128 * GIB)),
        2 * 1024 * MIB
    );
}

/// A job target this build's daemon serves, and the label its cache keys carry.
#[cfg(feature = "wasm-backend")]
fn served_target() -> (DaemonJobTarget, &'static str) {
    (DaemonJobTarget::Wasm(WasmTargetOptions::default()), "wasm")
}

#[cfg(not(feature = "wasm-backend"))]
fn served_target() -> (DaemonJobTarget, &'static str) {
    (
        DaemonJobTarget::Native(NativeTargetOptions::default()),
        "object",
    )
}

#[test]
fn daemon_probe_cache_only_returns_needs_ir_on_miss() {
    let _env_guard = TestEnvGuard::clear(SHARED_STDLIB_CACHE_ENV_KEYS);
    let cache = DaemonCache::new(None);
    let result = compile_single_job(
        DaemonJobRequest {
            id: "job0".to_string(),
            target: served_target().0,
            output: "/tmp/unused.o".to_string(),
            cache_key: "module".to_string(),
            function_cache_key: Some("function".to_string()),
            skip_module_output_if_synced: false,
            skip_function_output_if_synced: false,
            probe_cache_only: true,
            ir: None,
            ir_path: None,
            config_namespace: None,
        },
        &cache,
    );

    assert!(result.ok);
    assert!(!result.cached);
    assert!(result.needs_ir);
    assert!(!result.output_written);
}

#[test]
fn daemon_probe_cache_only_hits_without_ir() {
    let _env_guard = TestEnvGuard::clear(SHARED_STDLIB_CACHE_ENV_KEYS);
    let cache = DaemonCache::new(None);
    cache.insert(
        format!("artifact-v1:{}:module", served_target().1),
        Arc::from(vec![1_u8, 2, 3]),
    );
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let output = std::env::temp_dir().join(format!("molt-backend-probe-hit-{nonce}.wasm"));

    let result = compile_single_job(
        DaemonJobRequest {
            id: "job0".to_string(),
            target: served_target().0,
            output: output.to_string_lossy().into_owned(),
            cache_key: "module".to_string(),
            function_cache_key: Some("function".to_string()),
            skip_module_output_if_synced: false,
            skip_function_output_if_synced: false,
            probe_cache_only: true,
            ir: None,
            ir_path: None,
            config_namespace: None,
        },
        &cache,
    );

    assert!(result.ok);
    assert!(result.cached);
    assert!(!result.needs_ir);
    assert!(output.exists());
    let _ = std::fs::remove_file(output);
}

#[test]
#[cfg(feature = "native-backend")]
fn daemon_cache_hit_requires_matching_shared_stdlib_artifact() {
    let _env_guard = TestEnvGuard::capture(SHARED_STDLIB_CACHE_ENV_KEYS);

    let cache = DaemonCache::new(None);
    cache.insert(
        "artifact-v1:archive:module".to_string(),
        Arc::from(vec![1_u8, 2, 3]),
    );
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("molt-backend-stdlib-cache-{nonce}"));
    let output = root.join("probe-hit.a");
    let missing_stdlib = root.join("missing-stdlib.a");
    std::fs::create_dir_all(&root).expect("create temp dir");
    unsafe {
        std::env::set_var("MOLT_STDLIB_OBJ", &missing_stdlib);
        std::env::set_var("MOLT_STDLIB_CACHE_KEY", "stdlib-key");
        std::env::set_var("MOLT_STDLIB_CACHE_MANIFEST", "stdlib-manifest");
    }

    let result = compile_single_job(
        DaemonJobRequest {
            id: "job0".to_string(),
            target: DaemonJobTarget::Native(NativeTargetOptions {
                target_triple: None,
                output_kind: NativeArtifactKind::Archive,
            }),
            output: output.to_string_lossy().into_owned(),
            cache_key: "module".to_string(),
            function_cache_key: Some("function".to_string()),
            skip_module_output_if_synced: false,
            skip_function_output_if_synced: false,
            probe_cache_only: true,
            ir: None,
            ir_path: None,
            config_namespace: None,
        },
        &cache,
    );

    assert!(result.ok);
    assert!(!result.cached);
    assert!(result.needs_ir);
    assert!(!output.exists());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn daemon_diagnostics_bypass_warm_module_and_function_cache_entries() {
    let _env = TestEnvGuard::clear(&DAEMON_REQUEST_ENV_KEYS);
    let root = std::env::temp_dir().join(format!(
        "molt-daemon-diagnostic-cache-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("create fixture");
    // The artifact kinds this build's backends serve.
    let served: &[(&str, bool)] = &[
        #[cfg(feature = "wasm-backend")]
        ("wasm", true),
        #[cfg(feature = "native-backend")]
        ("object", false),
        #[cfg(feature = "native-backend")]
        ("archive", false),
    ];
    for &(kind, is_wasm) in served {
        for tier in ["module", "function"] {
            let cache = DaemonCache::new(None);
            cache.insert(
                format!("artifact-v1:{kind}:{tier}"),
                Arc::from(b"cached output".as_slice()),
            );
            for (case, dump, timing) in [
                ("ordinary", None, None),
                ("timing", None, Some("1")),
                ("empty-dump", Some(""), None),
                ("dump", Some("1"), None),
            ] {
                for (key, value) in [("MOLT_DUMP_IR", dump), ("MOLT_BACKEND_TIMING", timing)] {
                    match value {
                        Some(value) => unsafe { std::env::set_var(key, value) },
                        None => unsafe { std::env::remove_var(key) },
                    }
                }
                let output = root.join(format!("{kind}-{tier}-{case}"));
                let mut request = serde_json::json!({
                    "id": case, "is_wasm": is_wasm,
                    "output": output.to_string_lossy(),
                    "cache_key": "module", "function_cache_key": "function",
                    "probe_cache_only": true,
                });
                if !is_wasm {
                    request["native_output_kind"] = kind.into();
                }
                let result = compile_single_job(
                    DaemonJobRequest::from_json_value(&request, "job").expect("parse probe"),
                    &cache,
                );
                let expected_hit = dump.is_none();
                assert!(result.ok, "{kind}/{tier}/{case}: {:?}", result.message);
                assert_eq!(result.cached, expected_hit, "{kind}/{tier}/{case}");
                assert_eq!(result.needs_ir, !expected_hit, "{kind}/{tier}/{case}");
                assert_eq!(result.output_written, expected_hit, "{kind}/{tier}/{case}");
                assert_eq!(output.exists(), expected_hit, "{kind}/{tier}/{case}");
                if expected_hit {
                    assert_eq!(
                        std::fs::read(&output).expect("cached output"),
                        b"cached output"
                    );
                }
            }
        }
    }
    std::fs::remove_dir_all(root).expect("clean fixture");
}

#[test]
#[cfg(all(molt_backend_daemon, feature = "native-backend"))]
fn concurrent_requests_for_one_artifact_compile_it_once() {
    // Request scopes carry the settings; the guard serializes this test with
    // the ones that write process settings.
    let _env_guard = TestEnvGuard::clear(&DAEMON_REQUEST_ENV_KEYS);
    const BUILDS: usize = 8;
    let root = std::env::temp_dir().join(format!(
        "molt-daemon-single-flight-{}-{}",
        std::process::id(),
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("create fixture");
    let request = |output: &std::path::Path| {
        serde_json::json!({
            "version": BACKEND_DAEMON_PROTOCOL_VERSION,
            "config_digest": "single-flight",
            "env": {},
            "jobs": [{
                "id": "job0",
                "is_wasm": false,
                "native_output_kind": "archive",
                "output": output.to_string_lossy(),
                "cache_key": "module",
                "function_cache_key": "function",
                "ir": {
                    "functions": [
                        {"return_abi": "void", "name": "molt_main", "params": [], "ops": [
                            {"kind": "call", "s_value": "helper"}, {"kind": "ret_void"}
                        ]},
                        {"return_abi": "void", "name": "helper", "params": [], "ops": [{"kind": "ret_void"}]}
                    ],
                    "profile": null
                }
            }]
        })
    };
    let cache = Arc::new(DaemonCache::new(None));
    let start = Arc::new(std::sync::Barrier::new(BUILDS));
    let builds: Vec<_> = (0..BUILDS)
        .map(|index| {
            let output = root.join(format!("app-{index}.a"));
            let parsed = DaemonRequest::from_json_bytes(
                &serde_json::to_vec(&request(&output)).expect("serialize request"),
            )
            .expect("parse request");
            let (cache, start) = (Arc::clone(&cache), Arc::clone(&start));
            std::thread::spawn(move || {
                let _scope = molt_ir::backend_environment::enter(parsed.environment);
                let job = parsed.jobs.expect("jobs").pop().expect("job");
                start.wait();
                let response = compile_single_job(job, &cache);
                (response, std::fs::read(&output).expect("read build output"))
            })
        })
        .collect();
    let results: Vec<_> = builds
        .into_iter()
        .map(|build| build.join().expect("build thread"))
        .collect();
    for (response, _) in &results {
        assert!(response.ok, "build failed: {:?}", response.message);
    }
    let compiled = results
        .iter()
        .filter(|(response, _)| !response.cached)
        .count();
    assert_eq!(
        compiled, 1,
        "concurrent builds compiled one artifact {compiled} times"
    );
    assert!(
        results.windows(2).all(|pair| pair[0].1 == pair[1].1),
        "builds wrote different bytes for one artifact"
    );
    std::fs::remove_dir_all(root).expect("remove fixture");
}
