use super::*;

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
                let mut cache = DaemonCache::new(None);
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
                    &mut cache,
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
    let mut cache = DaemonCache::new(None);
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
        &mut cache,
    );
    assert!(result.ok && result.cached && result.output_written);
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
            let mut cache = DaemonCache::new(None);
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
            let result = compile_single_job(job, &mut cache);
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
    let mut cache = DaemonCache::new(None);
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
        &mut cache,
    );
    assert!(result.ok && result.cached && result.output_written);
    assert_eq!(
        std::fs::read(output).expect("wasm cache replay"),
        b"wasm cache"
    );
    std::fs::remove_dir_all(root).expect("remove fixture");
}

#[test]
#[cfg(feature = "native-backend")]
fn daemon_cache_artifact_kinds_cannot_alias_even_for_equal_caller_keys() {
    let _env = TestEnvGuard::clear(SHARED_STDLIB_CACHE_ENV_KEYS);
    let mut cache = DaemonCache::new(None);
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
        let miss = compile_single_job(job, &mut cache);
        assert!(
            miss.needs_ir,
            "other artifact kinds cannot satisfy this cache probe"
        );
        cache.insert(
            format!("artifact-v1:{kind}:same-functions"),
            Arc::from(expected),
        );
        let job = DaemonJobRequest::from_json_value(&request, "job").expect("parse replay job");
        let hit = compile_single_job(job, &mut cache);
        assert!(hit.cached && hit.output_written);
        assert_eq!(std::fs::read(&output).expect("read replay"), expected);
    }
    std::fs::remove_dir_all(root).expect("clean test directory");
}

#[test]
fn daemon_cache_get_bytes_updates_lru_without_cloning() {
    let mut cache = DaemonCache::new(None);
    cache.insert("module".to_string(), Arc::from(vec![1, 2, 3, 4]));

    let bytes = cache.get_bytes("module").expect("cache hit");
    assert_eq!(bytes, &[1, 2, 3, 4]);

    let entry = cache.entries.get("module").expect("entry retained");
    assert_eq!(entry.bytes.as_ref(), &[1, 2, 3, 4]);
    assert_eq!(entry.stamp, cache.clock);
}

#[test]
fn daemon_cache_can_share_bytes_across_keys() {
    let mut cache = DaemonCache::new(None);
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

#[test]
fn daemon_probe_cache_only_returns_needs_ir_on_miss() {
    let _env_guard = TestEnvGuard::clear(SHARED_STDLIB_CACHE_ENV_KEYS);
    let mut cache = DaemonCache::new(None);
    let result = compile_single_job(
        DaemonJobRequest {
            id: "job0".to_string(),
            is_wasm: true,
            target_triple: None,
            native_output_kind: NativeArtifactKind::Object,
            wasm_link: false,
            wasm_data_base: None,
            wasm_table_base: None,
            wasm_split_runtime_app_table_base: None,
            output: "/tmp/unused.o".to_string(),
            cache_key: "module".to_string(),
            function_cache_key: Some("function".to_string()),
            skip_module_output_if_synced: false,
            skip_function_output_if_synced: false,
            probe_cache_only: true,
            ir: None,
            ir_path: None,
        },
        &mut cache,
    );

    assert!(result.ok);
    assert!(!result.cached);
    assert!(result.needs_ir);
    assert!(!result.output_written);
}

#[test]
fn daemon_probe_cache_only_hits_without_ir() {
    let _env_guard = TestEnvGuard::clear(SHARED_STDLIB_CACHE_ENV_KEYS);
    let mut cache = DaemonCache::new(None);
    cache.insert(
        "artifact-v1:wasm:module".to_string(),
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
            is_wasm: true,
            target_triple: None,
            native_output_kind: NativeArtifactKind::Object,
            wasm_link: false,
            wasm_data_base: None,
            wasm_table_base: None,
            wasm_split_runtime_app_table_base: None,
            output: output.to_string_lossy().into_owned(),
            cache_key: "module".to_string(),
            function_cache_key: Some("function".to_string()),
            skip_module_output_if_synced: false,
            skip_function_output_if_synced: false,
            probe_cache_only: true,
            ir: None,
            ir_path: None,
        },
        &mut cache,
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

    let mut cache = DaemonCache::new(None);
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
            is_wasm: false,
            target_triple: None,
            native_output_kind: NativeArtifactKind::Archive,
            wasm_link: false,
            wasm_data_base: None,
            wasm_table_base: None,
            wasm_split_runtime_app_table_base: None,
            output: output.to_string_lossy().into_owned(),
            cache_key: "module".to_string(),
            function_cache_key: Some("function".to_string()),
            skip_module_output_if_synced: false,
            skip_function_output_if_synced: false,
            probe_cache_only: true,
            ir: None,
            ir_path: None,
        },
        &mut cache,
    );

    assert!(result.ok);
    assert!(!result.cached);
    assert!(result.needs_ir);
    assert!(!output.exists());
    let _ = std::fs::remove_dir_all(root);
}
