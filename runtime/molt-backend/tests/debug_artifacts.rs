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

#[test]
fn prepare_unique_debug_artifact_path_creates_unique_sibling_paths() {
    let base = unique_base("unique");
    let prior = std::env::var_os("MOLT_EXT_ROOT");
    unsafe {
        std::env::set_var("MOLT_EXT_ROOT", &base);
    }
    let a =
        molt_backend::debug_artifacts::prepare_unique_debug_artifact_path("llvm/output.o").unwrap();
    let b =
        molt_backend::debug_artifacts::prepare_unique_debug_artifact_path("llvm/output.o").unwrap();
    assert_ne!(a, b);
    assert_eq!(a.parent(), b.parent());
    assert_eq!(a.parent(), Some(base.join("tmp").join("molt-backend").join("llvm").as_path()));
    assert!(a.file_name().unwrap().to_string_lossy().starts_with("output."));
    assert!(b.file_name().unwrap().to_string_lossy().starts_with("output."));
    match prior {
        Some(value) => unsafe { std::env::set_var("MOLT_EXT_ROOT", value) },
        None => unsafe { std::env::remove_var("MOLT_EXT_ROOT") },
    }
}
