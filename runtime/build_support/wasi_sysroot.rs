use std::env;
use std::path::{Path, PathBuf};

const WASI_TARGET_INCLUDE_DIRS: &[&str] = &["wasm32-wasip1", "wasm32-wasi"];

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WasiSysroot {
    root: PathBuf,
    include_dir: Option<PathBuf>,
    lib_dir: Option<PathBuf>,
}

impl WasiSysroot {
    pub fn include_dir(&self) -> Option<&Path> {
        self.include_dir.as_deref()
    }

    pub fn lib_dir(&self, preferred_target: &str) -> PathBuf {
        self.lib_dir
            .clone()
            .unwrap_or_else(|| self.root.join("lib").join(preferred_target))
    }

    pub fn sysroot_flag(&self) -> String {
        format!("--sysroot={}", self.root.display())
    }
}

fn wasi_sdk_sysroot_candidates(raw: &str) -> Vec<PathBuf> {
    let sdk_root = PathBuf::from(raw);
    vec![
        sdk_root.clone(),
        sdk_root.join("share").join("wasi-sysroot"),
        sdk_root.join("wasi-sysroot"),
    ]
}

fn target_include_layout(root: &Path, target: &str) -> Option<WasiSysroot> {
    let include_dir = root.join("include").join(target);
    if !include_dir.join("errno.h").exists() {
        return None;
    }
    let lib_dir = root.join("lib").join(target);
    Some(WasiSysroot {
        root: root.to_path_buf(),
        include_dir: Some(include_dir),
        lib_dir: lib_dir.exists().then_some(lib_dir),
    })
}

fn normalize_target_include_path(path: &Path) -> Option<WasiSysroot> {
    let target = path.file_name().and_then(|name| name.to_str())?;
    if !WASI_TARGET_INCLUDE_DIRS.contains(&target) {
        return None;
    }
    if path
        .parent()
        .and_then(|parent| parent.file_name())
        .and_then(|name| name.to_str())
        != Some("include")
    {
        return None;
    }
    if !path.join("errno.h").exists() {
        return None;
    }
    let root = path.parent().and_then(|parent| parent.parent())?;
    let lib_dir = root.join("lib").join(target);
    Some(WasiSysroot {
        root: root.to_path_buf(),
        include_dir: Some(path.to_path_buf()),
        lib_dir: lib_dir.exists().then_some(lib_dir),
    })
}

fn normalize_wasi_sysroot(path: PathBuf) -> Option<WasiSysroot> {
    if let Some(layout) = normalize_target_include_path(&path) {
        return Some(layout);
    }
    let mut roots = vec![path.clone()];
    if path.file_name().and_then(|name| name.to_str()) == Some("include") {
        if let Some(parent) = path.parent() {
            roots.push(parent.to_path_buf());
        }
    }
    for root in roots {
        for target in WASI_TARGET_INCLUDE_DIRS {
            if let Some(layout) = target_include_layout(&root, target) {
                return Some(layout);
            }
        }
        if root.join("include").join("errno.h").exists() {
            let lib_dir = WASI_TARGET_INCLUDE_DIRS
                .iter()
                .map(|target| root.join("lib").join(target))
                .find(|candidate| candidate.exists());
            return Some(WasiSysroot {
                root,
                include_dir: None,
                lib_dir,
            });
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

pub fn resolve_wasi_sysroot() -> Option<WasiSysroot> {
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
