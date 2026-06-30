use std::env;
use std::path::PathBuf;

fn wasi_sdk_sysroot_candidates(raw: &str) -> Vec<PathBuf> {
    let sdk_root = PathBuf::from(raw);
    vec![
        sdk_root.clone(),
        sdk_root.join("share").join("wasi-sysroot"),
        sdk_root.join("wasi-sysroot"),
    ]
}

fn normalize_wasi_sysroot(path: PathBuf) -> Option<PathBuf> {
    let mut roots = vec![path.clone()];
    if path.file_name().and_then(|name| name.to_str()) == Some("include") {
        if let Some(parent) = path.parent() {
            roots.push(parent.to_path_buf());
        }
    }
    if path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        == Some("include")
        && path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with("wasm32-"))
    {
        if let Some(parent) = path.parent().and_then(|parent| parent.parent()) {
            roots.push(parent.to_path_buf());
        }
    }
    for root in roots {
        if root.join("include").join("errno.h").exists()
            || root
                .join("include")
                .join("wasm32-wasip1")
                .join("errno.h")
                .exists()
            || root
                .join("include")
                .join("wasm32-wasi")
                .join("errno.h")
                .exists()
        {
            return Some(root);
        }
    }
    None
}

fn push_target_root_candidates(candidates: &mut Vec<PathBuf>, raw: &str) {
    let target_root = PathBuf::from(raw);
    candidates.extend([
        target_root.join("toolchains").join("wasi-sysroot"),
        target_root
            .join("toolchains")
            .join("wasi-sdk")
            .join("share")
            .join("wasi-sysroot"),
        target_root
            .join("toolchains")
            .join("wasi-sdk")
            .join("wasi-sysroot"),
        target_root.join("wasi-sysroot"),
        target_root
            .join("wasi-sdk")
            .join("share")
            .join("wasi-sysroot"),
        target_root.join("wasi-sdk").join("wasi-sysroot"),
    ]);
    if let Ok(entries) = std::fs::read_dir(target_root.join("toolchains")) {
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            if let Some(name) = path.file_name().and_then(|name| name.to_str()) {
                if name.starts_with("wasi-sysroot") {
                    candidates.push(path.clone());
                }
                if name.starts_with("wasi-sdk") {
                    candidates.push(path.join("share").join("wasi-sysroot"));
                    candidates.push(path.join("wasi-sysroot"));
                }
            }
        }
    }
}

fn wasi_sysroot_candidates() -> Vec<PathBuf> {
    let mut candidates = Vec::new();
    for key in ["MOLT_WASI_SYSROOT", "WASI_SYSROOT"] {
        if let Ok(value) = env::var(key) {
            candidates.push(PathBuf::from(value));
        }
    }
    for key in ["WASI_SDK_PATH", "WASI_SDK_PREFIX"] {
        if let Ok(value) = env::var(key) {
            candidates.extend(wasi_sdk_sysroot_candidates(&value));
        }
    }
    if let Ok(value) = env::var("MOLT_TARGET_ROOT") {
        push_target_root_candidates(&mut candidates, &value);
    }
    candidates.extend([
        PathBuf::from("/opt/homebrew/opt/wasi-libc/share/wasi-sysroot"),
        PathBuf::from("/usr/local/opt/wasi-libc/share/wasi-sysroot"),
        PathBuf::from("/opt/wasi-sdk/share/wasi-sysroot"),
        PathBuf::from("/opt/wasi-sdk/wasi-sysroot"),
        PathBuf::from("/usr/share/wasi-sysroot"),
        PathBuf::from("/usr/include/wasm32-wasi"),
        PathBuf::from("/usr/local/share/wasi-sysroot"),
        PathBuf::from("/usr/local/include/wasm32-wasi"),
    ]);
    candidates
}

pub fn resolve_wasi_sysroot() -> Option<PathBuf> {
    for key in [
        "MOLT_WASI_SYSROOT",
        "WASI_SYSROOT",
        "WASI_SDK_PATH",
        "WASI_SDK_PREFIX",
        "MOLT_TARGET_ROOT",
    ] {
        println!("cargo:rerun-if-env-changed={key}");
    }
    let mut seen = Vec::new();
    for candidate in wasi_sysroot_candidates() {
        if seen.iter().any(|path: &PathBuf| path == &candidate) {
            continue;
        }
        seen.push(candidate.clone());
        if let Some(root) = normalize_wasi_sysroot(candidate) {
            return Some(root);
        }
    }
    None
}
