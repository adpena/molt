use super::*;
use std::sync::Arc;

const SHARED_STDLIB_CACHE_ENV_KEYS: &[&str] = &[
    "MOLT_STDLIB_OBJ",
    "MOLT_STDLIB_CACHE_ENTRY",
    "MOLT_STDLIB_CACHE_KEY",
    "MOLT_STDLIB_CACHE_MANIFEST",
];

fn cache_fixture_job(target: DaemonJobTarget, source: &str) -> DaemonJobRequest {
    DaemonJobRequest {
        id: "cache-fixture".to_string(),
        target,
        output: String::new(),
        cache_key: source.to_string(),
        function_cache_key: None,
        skip_module_output_if_synced: false,
        skip_function_output_if_synced: false,
        probe_cache_only: true,
        ir: None,
        ir_path: None,
        config_namespace: None,
    }
}

/// Seed only the byte-store boundary in admission tests. The target-isolation
/// controls below populate their entries through actual compilation instead.
fn seed_daemon_cache(cache: &DaemonCache, kind: &str, source: &str, bytes: &[u8]) {
    let value = serde_json::json!({
        "id": "cache-fixture", "output": "unused", "cache_key": source,
        "is_wasm": kind == "wasm",
        "native_output_kind": if kind == "wasm" { None } else { Some(kind) },
    });
    let mut job = DaemonJobRequest::from_json_value(&value, "fixture").unwrap();
    let (key, _) = job.take_artifact_cache_keys();
    cache.insert(key.expect("nonempty fixture key"), Arc::from(bytes));
}

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
                    seed_daemon_cache(&cache, kind, "same", b"cached");
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
        seed_daemon_cache(&cache, "wasm", "same", b"wasm");
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
                seed_daemon_cache(&cache, "object", "same", b"cached object");
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
        seed_daemon_cache(&cache, "wasm", "same", b"wasm cache");
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
        seed_daemon_cache(&cache, kind, "same-functions", expected);
        let job = DaemonJobRequest::from_json_value(&request, "job").expect("parse replay job");
        let hit = compile_single_job(job, &cache);
        assert!(hit.cached && hit.output_written);
        assert_eq!(std::fs::read(&output).expect("read replay"), expected);
    }
    std::fs::remove_dir_all(root).expect("clean test directory");
}

/// Exercise both producers and both lookup tiers. Equal caller keys and
/// namespaces are deliberate: the daemon must own target isolation itself.
fn assert_compiled_cache_target_isolation(
    target_fields: serde_json::Value,
    alternatives: &[(&str, serde_json::Value)],
) {
    let root = molt_ir::debug_artifacts::prepare_unique_debug_artifact_path("daemon-target-cache")
        .expect("unique fixture path");
    std::fs::create_dir(&root).expect("create fixture");
    for tier in ["module", "function"] {
        let cache = DaemonCache::new(None);
        let original = root.join(format!("{tier}-original"));
        let mut request = target_fields.clone();
        request["id"] = "original".into();
        request["output"] = original.to_str().unwrap().into();
        request["cache_key"] = if tier == "module" { "same" } else { "" }.into();
        request["function_cache_key"] = if tier == "function" { "same" } else { "" }.into();
        request["ir"] = serde_json::json!({
            "functions": [{
                "return_abi": "void", "name": "molt_main", "params": [],
                "ops": [{"kind": "ret_void"}]
            }],
            "profile": null
        });
        let compile = |value: &serde_json::Value| {
            let mut job = DaemonJobRequest::from_json_value(value, "job").expect("parse job");
            job.config_namespace = Some("same-config".to_string());
            compile_single_job(job, &cache)
        };
        let built = compile(&request);
        assert!(
            built.ok && !built.cached && built.output_written,
            "{tier}: cold compilation failed: {:?}",
            built.message
        );
        let expected = std::fs::read(&original).expect("compiled output");
        assert!(!expected.is_empty());
        let missing_ir = root.join("absent-ir.json");
        assert!(!missing_ir.exists());
        request.as_object_mut().unwrap().remove("ir");
        request["ir_path"] = missing_ir.to_str().unwrap().into();
        request["probe_cache_only"] = true.into();
        for (index, (field, value)) in alternatives.iter().enumerate() {
            let output = root.join(format!("{tier}-different-{index}"));
            let mut changed = request.clone();
            changed[*field] = value.clone();
            changed["output"] = output.to_str().unwrap().into();
            let miss = compile(&changed);
            assert!(
                miss.ok && miss.needs_ir && !miss.cached && !miss.output_written,
                "{tier}/{field}={value}: different target reused cached bytes: {miss:?}"
            );
            assert!(!output.exists(), "a miss must not publish output");
        }
        // The changed-target probes must neither evict nor replace the
        // producer's entry. A different output/id is still the same artifact.
        let replay = root.join(format!("{tier}-same-target"));
        request["id"] = "replay".into();
        request["output"] = replay.to_str().unwrap().into();
        let hit = compile(&request);
        assert!(hit.ok && hit.cached && !hit.needs_ir && hit.output_written);
        assert_eq!(std::fs::read(replay).unwrap(), expected);
        assert!(!missing_ir.exists());
        assert_eq!(cache.usage().0, 1, "only the actual producer inserts");
    }
    std::fs::remove_dir_all(root).expect("remove fixture");
}

#[cfg(feature = "native-backend")]
#[test]
fn daemon_compiled_native_cache_isolates_every_target_option_and_lookup_tier() {
    let _env = TestEnvGuard::clear(&DAEMON_REQUEST_ENV_KEYS);
    assert_compiled_cache_target_isolation(
        serde_json::json!({"is_wasm": false, "native_output_kind": "object"}),
        &[
            ("target_triple", "aarch64-unknown-linux-gnu".into()),
            ("target_triple", "x86_64-pc-windows-msvc".into()),
            ("native_output_kind", "archive".into()),
        ],
    );
}

#[cfg(feature = "wasm-backend")]
#[test]
fn daemon_compiled_wasm_cache_isolates_every_target_option_and_lookup_tier() {
    let _env = TestEnvGuard::clear(&DAEMON_REQUEST_ENV_KEYS);
    assert_compiled_cache_target_isolation(
        serde_json::json!({"is_wasm": true}),
        &[
            ("wasm_link", true.into()),
            ("wasm_data_base", 1_048_576.into()),
            ("wasm_data_base", 0.into()),
            ("wasm_table_base", 4096.into()),
            ("wasm_split_runtime_app_table_base", 8192.into()),
        ],
    );
}

#[test]
fn daemon_cache_get_bytes_updates_lru_without_cloning() {
    let mut cache = DaemonLru::new(None);
    let (key, _) = cache_fixture_job(served_target(), "module").take_artifact_cache_keys();
    let key = key.unwrap();
    cache.insert(Arc::clone(&key), Arc::from(vec![1, 2, 3, 4]));

    let bytes = cache.get_bytes(&key).expect("cache hit");
    assert_eq!(bytes.as_ref(), &[1, 2, 3, 4]);

    let entry = cache.entries.get(&key).expect("entry retained");
    assert_eq!(entry.bytes.as_ref(), &[1, 2, 3, 4]);
    assert_eq!(entry.stamp, cache.clock);
}

#[test]
fn daemon_cache_can_share_bytes_across_keys() {
    let mut cache = DaemonLru::new(None);
    let shared = Arc::<[u8]>::from(vec![9, 8, 7, 6]);
    let (module_key, _) = cache_fixture_job(served_target(), "module").take_artifact_cache_keys();
    let (function_key, _) =
        cache_fixture_job(served_target(), "function").take_artifact_cache_keys();
    let module_key = module_key.unwrap();
    let function_key = function_key.unwrap();
    cache.insert(Arc::clone(&module_key), Arc::clone(&shared));
    cache.insert(Arc::clone(&function_key), shared);

    let module = cache.entries.get(&module_key).expect("module entry");
    let function = cache.entries.get(&function_key).expect("function entry");
    assert!(Arc::ptr_eq(&module.bytes, &function.bytes));
}

#[test]
fn daemon_hit_only_cache_bounds_order_memory_and_preserves_eviction() {
    let mut cache = DaemonLru::new(Some(8));
    let (first, _) = cache_fixture_job(served_target(), "first").take_artifact_cache_keys();
    let (second, _) = cache_fixture_job(served_target(), "second").take_artifact_cache_keys();
    let (third, _) = cache_fixture_job(served_target(), "third").take_artifact_cache_keys();
    let (first, second, third) = (first.unwrap(), second.unwrap(), third.unwrap());
    let payload = Arc::<[u8]>::from([1, 2, 3, 4].as_slice());
    cache.insert(Arc::clone(&first), Arc::clone(&payload));
    cache.insert(Arc::clone(&second), Arc::from([5, 6, 7, 8].as_slice()));
    for _ in 0..1024 {
        let hit = cache.get_bytes(&first).expect("warm first entry");
        assert!(Arc::ptr_eq(&hit, &payload));
        assert!(
            cache.order.len() <= 64,
            "hit-only traffic must not retain a generation for every lookup"
        );
    }
    assert_eq!(cache.entries.len(), 2);
    assert_eq!(cache.bytes, 8);
    cache.insert(Arc::clone(&third), Arc::from([9, 10, 11, 12].as_slice()));
    assert!(cache.get_bytes(&second).is_none(), "evict the cold entry");
    assert!(Arc::ptr_eq(&cache.get_bytes(&first).unwrap(), &payload));
    assert_eq!(cache.get_bytes(&third).unwrap().as_ref(), &[9, 10, 11, 12]);
    assert_eq!(cache.entries.len(), 2);
    assert_eq!(cache.bytes, 8);
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

/// A job target this build's daemon serves.
#[cfg(feature = "wasm-backend")]
fn served_target() -> DaemonJobTarget {
    DaemonJobTarget::Wasm(WasmTargetOptions::default())
}

#[cfg(not(feature = "wasm-backend"))]
fn served_target() -> DaemonJobTarget {
    DaemonJobTarget::Native(NativeTargetOptions::default())
}

#[test]
fn daemon_probe_cache_only_returns_needs_ir_on_miss() {
    let _env_guard = TestEnvGuard::clear(SHARED_STDLIB_CACHE_ENV_KEYS);
    let missing_lease =
        molt_ir::debug_artifacts::prepare_unique_debug_artifact_path("probe-missing-ir")
            .expect("unique absent lease");
    assert!(!missing_lease.exists());
    let cache = DaemonCache::new(None);
    let result = compile_single_job(
        DaemonJobRequest {
            id: "job0".to_string(),
            target: served_target(),
            output: "/tmp/unused.o".to_string(),
            cache_key: "module".to_string(),
            function_cache_key: Some("function".to_string()),
            skip_module_output_if_synced: false,
            skip_function_output_if_synced: false,
            probe_cache_only: true,
            ir: None,
            ir_path: Some(missing_lease.to_string_lossy().into_owned()),
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
fn daemon_probe_cache_only_hits_without_reading_ir() {
    let _env_guard = TestEnvGuard::clear(SHARED_STDLIB_CACHE_ENV_KEYS);
    let cache = DaemonCache::new(None);
    let (key, _) = cache_fixture_job(served_target(), "module").take_artifact_cache_keys();
    cache.insert(key.unwrap(), Arc::from(vec![1_u8, 2, 3]));
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let output = std::env::temp_dir().join(format!("molt-backend-probe-hit-{nonce}.wasm"));

    let missing_lease = output.with_extension("missing-ir");
    assert!(!missing_lease.exists());
    for ir_path in [None, Some(missing_lease.to_string_lossy().into_owned())] {
        let result = compile_single_job(
            DaemonJobRequest {
                id: "job0".to_string(),
                target: served_target(),
                output: output.to_string_lossy().into_owned(),
                cache_key: "module".to_string(),
                function_cache_key: Some("function".to_string()),
                skip_module_output_if_synced: false,
                skip_function_output_if_synced: false,
                probe_cache_only: true,
                ir: None,
                ir_path,
                config_namespace: None,
            },
            &cache,
        );

        assert!(result.ok);
        assert!(result.cached);
        assert!(!result.needs_ir);
        assert!(output.exists());
    }
    let _ = std::fs::remove_file(output);
}

#[test]
#[cfg(feature = "native-backend")]
fn daemon_cache_hit_requires_matching_shared_stdlib_artifact() {
    let _env_guard = TestEnvGuard::capture(SHARED_STDLIB_CACHE_ENV_KEYS);

    let cache = DaemonCache::new(None);
    seed_daemon_cache(&cache, "archive", "module", &[1_u8, 2, 3]);
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
            seed_daemon_cache(&cache, kind, tier, b"cached output");
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

#[cfg(feature = "native-backend")]
#[test]
fn daemon_json_lease_rejects_suffix_before_output_publication() {
    let _env_guard = TestEnvGuard::clear(SHARED_STDLIB_CACHE_ENV_KEYS);
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let root =
        std::env::temp_dir().join(format!("molt-daemon-frame-{}-{nonce}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let lease = root.join("input.json");
    let output = root.join("output.o");
    for bytes in [
        b"{\"functions\":[]}{}".as_slice(),
        b"{\"functions\":[]}!".as_slice(),
    ] {
        std::fs::write(&lease, bytes).unwrap();
        let job = DaemonJobRequest::from_json_value(
            &serde_json::json!({
                "id":"lease-frame", "is_wasm":false, "output":output.to_str().unwrap(),
                "cache_key":"", "ir_path":lease.to_str().unwrap(),
            }),
            "job",
        )
        .unwrap();
        let response = compile_single_job(job, &DaemonCache::new(None));
        assert!(!response.ok && !response.cached && !response.output_written);
        let error = response.message.expect("lease error");
        assert!(
            error.contains("ir_path") && error.contains("trailing"),
            "{error}"
        );
        assert!(!output.exists());
    }
    std::fs::remove_dir_all(root).unwrap();
}
