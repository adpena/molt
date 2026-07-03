use super::*;

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
            is_wasm: false,
            target_triple: None,
            wasm_link: false,
            wasm_data_base: None,
            wasm_table_base: None,
            wasm_split_runtime_runtime_table_min: None,
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
    cache.insert("module".to_string(), Arc::from(vec![1_u8, 2, 3]));
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let output = std::env::temp_dir().join(format!("molt-backend-probe-hit-{nonce}.o"));

    let result = compile_single_job(
        DaemonJobRequest {
            id: "job0".to_string(),
            is_wasm: false,
            target_triple: None,
            wasm_link: false,
            wasm_data_base: None,
            wasm_table_base: None,
            wasm_split_runtime_runtime_table_min: None,
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
fn daemon_cache_hit_requires_matching_shared_stdlib_artifact() {
    let _env_guard = TestEnvGuard::capture(SHARED_STDLIB_CACHE_ENV_KEYS);

    let mut cache = DaemonCache::new(None);
    cache.insert("module".to_string(), Arc::from(vec![1_u8, 2, 3]));
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("molt-backend-stdlib-cache-{nonce}"));
    let output = root.join("probe-hit.o");
    let missing_stdlib = root.join("missing-stdlib.o");
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
            wasm_link: false,
            wasm_data_base: None,
            wasm_table_base: None,
            wasm_split_runtime_runtime_table_min: None,
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
