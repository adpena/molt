use std::fs;
use std::path::PathBuf;

fn unique_base(name: &str) -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "molt-debug-artifacts-{name}-{}-{}",
        std::process::id(),
        std::thread::current().name().unwrap_or("main"),
    ));
    let _ = fs::remove_dir_all(&path);
    fs::create_dir_all(&path).unwrap();
    path
}

#[test]
fn repo_debug_artifact_root_lives_under_repo_tmp() {
    let base = unique_base("root");
    let root = molt_backend::debug_artifacts::repo_debug_artifact_root(&base);
    assert_eq!(root, base.join("tmp").join("molt-backend"));
}

#[test]
fn write_debug_artifact_under_creates_parent_dirs() {
    let base = unique_base("write");
    let path = molt_backend::debug_artifacts::write_debug_artifact_under(
        &base,
        "tir/roundtrip/example.txt",
        b"hello",
    )
    .unwrap();
    assert_eq!(
        path,
        base.join("tmp")
            .join("molt-backend")
            .join("tir")
            .join("roundtrip")
            .join("example.txt")
    );
    assert_eq!(fs::read(&path).unwrap(), b"hello");
}
