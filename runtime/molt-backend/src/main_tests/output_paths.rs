use super::*;

#[test]
fn write_cached_output_can_skip_disk_write_when_synced() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let output = std::env::temp_dir().join(format!("molt-backend-test-{nonce}.o"));

    let written = write_cached_output(output.to_str().expect("utf8 path"), b"artifact", true)
        .expect("cache hit succeeds");

    assert!(!written);
    assert!(!output.exists());
}

#[test]
fn ensure_output_parent_dir_creates_nested_directories() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("molt-backend-parent-{nonce}"));
    let output = root.join("nested").join("cache").join("artifact.wasm");

    ensure_output_parent_dir(output.to_str().expect("utf8 path")).expect("parent dir creation");

    assert!(output.parent().expect("parent exists").is_dir());
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn atomic_backend_output_recreates_missing_parent() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("molt-backend-create-{nonce}"));
    let output = root.join("nested").join("cache").join("artifact.o");

    ensure_output_parent_dir(output.to_str().expect("utf8 path")).expect("prime parent");
    std::fs::remove_dir_all(root.join("nested")).expect("remove parent tree");

    write_bytes_atomically(&output, b"artifact").expect("publish artifact");

    assert_eq!(std::fs::read(&output).expect("read artifact"), b"artifact");
    let _ = std::fs::remove_dir_all(root);
}

#[test]
fn default_backend_output_paths_use_dist_root() {
    assert_eq!(
        default_backend_output_path(BackendOutputKind::Luau),
        "dist/output.luau"
    );
    assert_eq!(
        default_backend_output_path(BackendOutputKind::Rust),
        "dist/output.rs"
    );
    assert_eq!(
        default_backend_output_path(BackendOutputKind::Wasm),
        "dist/output.wasm"
    );
    assert_eq!(
        default_backend_output_path(BackendOutputKind::Native),
        "dist/output.o"
    );
}

#[test]
fn resolve_backend_output_path_prefers_explicit_output() {
    let explicit = "/tmp/custom/output.wasm";
    assert_eq!(
        resolve_backend_output_path(Some(explicit), BackendOutputKind::Wasm),
        explicit
    );
    assert_eq!(
        resolve_backend_output_path(None, BackendOutputKind::Wasm),
        "dist/output.wasm"
    );
}

#[test]
fn backend_rss_default_scales_with_host_memory() {
    assert_eq!(
        default_backend_max_rss_gb_from_physical_mem_bytes(Some(8 * GIB)),
        4
    );
    assert_eq!(
        default_backend_max_rss_gb_from_physical_mem_bytes(Some(16 * GIB)),
        8
    );
    assert_eq!(
        default_backend_max_rss_gb_from_physical_mem_bytes(Some(32 * GIB)),
        12
    );
    assert_eq!(
        default_backend_max_rss_gb_from_physical_mem_bytes(Some(64 * GIB)),
        16
    );
}
