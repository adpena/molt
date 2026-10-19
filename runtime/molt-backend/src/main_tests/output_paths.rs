use super::*;
use molt_artifact_publish::write_bytes_atomically;

#[cfg(molt_backend_daemon)]
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

#[cfg(molt_backend_daemon)]
#[test]
fn write_cached_output_publishes_through_atomic_output_authority() {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("molt-backend-cache-publish-{nonce}"));
    let output = root.join("nested").join("artifact.wasm");

    let written = write_cached_output(
        output.to_str().expect("utf8 path"),
        b"cached artifact",
        false,
    )
    .expect("cache output publication succeeds");

    assert!(written);
    assert_eq!(
        std::fs::read(&output).expect("read cached output"),
        b"cached artifact"
    );
    let _ = std::fs::remove_dir_all(root);
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
    #[cfg(feature = "native-backend")]
    for (output_kind, path) in [
        (NativeArtifactKind::Object, "dist/output.o"),
        (NativeArtifactKind::Archive, "dist/output.a"),
    ] {
        let target = BackendTarget::Native(NativeTargetOptions {
            target_triple: None,
            output_kind,
        });
        assert_eq!(target.default_output_path(), path);
    }
    #[cfg(feature = "wasm-backend")]
    assert_eq!(
        BackendTarget::Wasm(WasmTargetOptions::default()).default_output_path(),
        "dist/output.wasm"
    );
    #[cfg(feature = "luau-backend")]
    assert_eq!(
        BackendTarget::Luau.default_output_path(),
        "dist/output.luau"
    );
    #[cfg(feature = "rust-backend")]
    assert_eq!(BackendTarget::Rust.default_output_path(), "dist/output.rs");
}
