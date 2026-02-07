use std::collections::BTreeMap;
use std::sync::atomic::AtomicU64;
use std::sync::{Mutex, OnceLock};

use crate::builtins::io::{
    path_basename_text, path_dirname_text, path_join_text, path_normpath_text,
};
#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
use crate::*;

// --- Platform constants ---

pub(crate) const IO_EVENT_READ: u32 = 1;
pub(crate) const IO_EVENT_WRITE: u32 = 1 << 1;
pub(crate) const IO_EVENT_ERROR: u32 = 1 << 2;

#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
pub(crate) const SOCK_NONBLOCK_FLAG: i32 = libc::SOCK_NONBLOCK;
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
pub(crate) const SOCK_NONBLOCK_FLAG: i32 = 0;
#[cfg(all(unix, any(target_os = "linux", target_os = "android")))]
pub(crate) const SOCK_CLOEXEC_FLAG: i32 = libc::SOCK_CLOEXEC;
#[cfg(all(unix, not(any(target_os = "linux", target_os = "android"))))]
pub(crate) const SOCK_CLOEXEC_FLAG: i32 = 0;

// --- errno/socket/env helpers ---

#[no_mangle]
pub extern "C" fn molt_bridge_unavailable(msg_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let msg = format_obj_str(_py, obj_from_bits(msg_bits));
        eprintln!("Molt bridge unavailable: {msg}");
        std::process::exit(1);
    })
}

static ERRNO_CONSTANTS_CACHE: AtomicU64 = AtomicU64::new(0);
static SOCKET_CONSTANTS_CACHE: AtomicU64 = AtomicU64::new(0);
static OS_NAME_CACHE: AtomicU64 = AtomicU64::new(0);
static SYS_PLATFORM_CACHE: AtomicU64 = AtomicU64::new(0);
static ENV_STATE: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
static PROCESS_ENV_STATE: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
static LOCALE_STATE: OnceLock<Mutex<String>> = OnceLock::new();

fn trace_env_get() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| {
        matches!(
            std::env::var("MOLT_TRACE_ENV_GET").ok().as_deref(),
            Some("1")
        )
    })
}

fn env_state() -> &'static Mutex<BTreeMap<String, String>> {
    ENV_STATE.get_or_init(|| Mutex::new(collect_env_state()))
}

fn process_env_state() -> &'static Mutex<BTreeMap<String, String>> {
    PROCESS_ENV_STATE.get_or_init(|| Mutex::new(collect_env_state()))
}

fn locale_state() -> &'static Mutex<String> {
    LOCALE_STATE.get_or_init(|| Mutex::new(String::from("C")))
}

fn collect_env_state() -> BTreeMap<String, String> {
    #[cfg(target_arch = "wasm32")]
    {
        collect_wasm_env_state()
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::vars().collect()
    }
}

#[cfg(target_arch = "wasm32")]
fn collect_wasm_env_state() -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let mut env_count = 0u32;
    let mut buf_size = 0u32;
    let rc = unsafe { environ_sizes_get(&mut env_count, &mut buf_size) };
    if rc != 0 || env_count == 0 || buf_size == 0 {
        return out;
    }
    let env_count = match usize::try_from(env_count) {
        Ok(val) => val,
        Err(_) => return out,
    };
    let buf_size = match usize::try_from(buf_size) {
        Ok(val) => val,
        Err(_) => return out,
    };
    let mut ptrs = vec![std::ptr::null_mut(); env_count];
    let mut buf = vec![0u8; buf_size];
    let rc = unsafe { environ_get(ptrs.as_mut_ptr(), buf.as_mut_ptr()) };
    if rc != 0 {
        return out;
    }
    for &ptr in &ptrs {
        if ptr.is_null() {
            continue;
        }
        let offset = (ptr as usize).saturating_sub(buf.as_ptr() as usize);
        if offset >= buf.len() {
            continue;
        }
        let slice = &buf[offset..];
        let end = slice.iter().position(|&b| b == 0).unwrap_or(slice.len());
        let entry = &slice[..end];
        let text = String::from_utf8_lossy(entry);
        if let Some((key, val)) = text.split_once('=') {
            out.insert(key.to_string(), val.to_string());
        }
    }
    out
}

fn os_name_str() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "nt"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "posix"
    }
}

fn sys_platform_str() -> &'static str {
    #[cfg(target_arch = "wasm32")]
    {
        "wasi"
    }
    #[cfg(all(not(target_arch = "wasm32"), target_os = "windows"))]
    {
        "win32"
    }
    #[cfg(all(not(target_arch = "wasm32"), target_os = "macos"))]
    {
        "darwin"
    }
    #[cfg(all(not(target_arch = "wasm32"), target_os = "linux"))]
    {
        "linux"
    }
    #[cfg(all(not(target_arch = "wasm32"), target_os = "android"))]
    {
        "android"
    }
    #[cfg(all(not(target_arch = "wasm32"), target_os = "freebsd"))]
    {
        "freebsd"
    }
    #[cfg(all(
        not(target_arch = "wasm32"),
        not(any(
            target_os = "windows",
            target_os = "macos",
            target_os = "linux",
            target_os = "android",
            target_os = "freebsd"
        ))
    ))]
    {
        "unknown"
    }
}

fn append_unique_path(paths: &mut Vec<String>, entry: &str) {
    if entry.is_empty() {
        return;
    }
    if paths.iter().any(|existing| existing == entry) {
        return;
    }
    paths.push(entry.to_string());
}

fn split_nonempty_paths(raw: &str, sep: char) -> Vec<String> {
    raw.split(sep)
        .filter_map(|part| {
            if part.is_empty() {
                None
            } else {
                Some(part.to_string())
            }
        })
        .collect()
}

fn resolve_bootstrap_pwd(raw_pwd: &str) -> String {
    if !raw_pwd.is_empty() {
        return raw_pwd.to_string();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if let Ok(cwd) = std::env::current_dir() {
            let text = cwd.to_string_lossy().into_owned();
            if !text.is_empty() {
                return text;
            }
        }
    }
    String::new()
}

fn path_is_dir(path: &str) -> bool {
    std::fs::metadata(path)
        .map(|metadata| metadata.is_dir())
        .unwrap_or(false)
}

fn collect_virtual_env_site_packages(virtual_env: &str, windows_paths: bool) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    if virtual_env.trim().is_empty() {
        return out;
    }
    let sep = if windows_paths { '\\' } else { '/' };
    let virtual_env = virtual_env.trim();
    if windows_paths {
        let lib = path_join_text(virtual_env.to_string(), "Lib", sep);
        let site_packages = path_join_text(lib, "site-packages", sep);
        if path_is_dir(&site_packages) {
            append_unique_path(&mut out, &site_packages);
        }
        let lib_lower = path_join_text(virtual_env.to_string(), "lib", sep);
        let site_packages_lower = path_join_text(lib_lower, "site-packages", sep);
        if path_is_dir(&site_packages_lower) {
            append_unique_path(&mut out, &site_packages_lower);
        }
        return out;
    }
    let lib = path_join_text(virtual_env.to_string(), "lib", sep);
    if !path_is_dir(&lib) {
        return out;
    }
    let mut discovered: Vec<String> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&lib) {
        for entry in entries.flatten() {
            let file_type = match entry.file_type() {
                Ok(kind) => kind,
                Err(_) => continue,
            };
            if !file_type.is_dir() {
                continue;
            }
            let dir_name = entry.file_name().to_string_lossy().into_owned();
            if !dir_name.starts_with("python") {
                continue;
            }
            let version_root = entry.path().to_string_lossy().into_owned();
            let candidate = path_join_text(version_root, "site-packages", sep);
            if path_is_dir(&candidate) {
                discovered.push(candidate);
            }
        }
    }
    discovered.sort();
    for candidate in discovered {
        append_unique_path(&mut out, &candidate);
    }
    let fallback = path_join_text(lib, "site-packages", sep);
    if path_is_dir(&fallback) {
        append_unique_path(&mut out, &fallback);
    }
    out
}

struct SysBootstrapState {
    path: Vec<String>,
    stdlib_root: Option<String>,
    pythonpath_entries: Vec<String>,
    module_roots_entries: Vec<String>,
    venv_site_packages_entries: Vec<String>,
    py_path_raw: String,
    module_roots_raw: String,
    virtual_env_raw: String,
    dev_trusted_raw: String,
    pwd: String,
    include_cwd: bool,
}

fn sys_bootstrap_state_from_module_file(module_file: Option<String>) -> SysBootstrapState {
    let (py_path_raw, module_roots_raw, virtual_env_raw, dev_trusted_raw, pwd_raw, windows_paths) = {
        let guard = env_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (
            guard.get("PYTHONPATH").cloned().unwrap_or_default(),
            guard.get("MOLT_MODULE_ROOTS").cloned().unwrap_or_default(),
            guard.get("VIRTUAL_ENV").cloned().unwrap_or_default(),
            guard.get("MOLT_DEV_TRUSTED").cloned().unwrap_or_default(),
            guard.get("PWD").cloned().unwrap_or_default(),
            sys_platform_str().starts_with("win"),
        )
    };

    let sep = if windows_paths { ';' } else { ':' };
    let pythonpath_entries = split_nonempty_paths(&py_path_raw, sep);
    let mut paths: Vec<String> = pythonpath_entries.clone();

    let stdlib_root = module_file.and_then(|path| {
        if path.is_empty() {
            return None;
        }
        let sep = if windows_paths { '\\' } else { '/' };
        let dirname = path_dirname_text(&path, sep);
        if dirname.is_empty() {
            None
        } else {
            Some(dirname)
        }
    });
    if let Some(root) = &stdlib_root {
        append_unique_path(&mut paths, root);
    }

    let mut module_roots_entries: Vec<String> = Vec::new();
    for entry in split_nonempty_paths(&module_roots_raw, sep) {
        append_unique_path(&mut module_roots_entries, &entry);
        append_unique_path(&mut paths, &entry);
    }

    let venv_site_packages_entries =
        collect_virtual_env_site_packages(&virtual_env_raw, windows_paths);
    for entry in &venv_site_packages_entries {
        append_unique_path(&mut paths, entry);
    }

    let dev_trusted = dev_trusted_raw.trim().to_ascii_lowercase();
    let include_cwd = !matches!(dev_trusted.as_str(), "0" | "false" | "no");
    let pwd = resolve_bootstrap_pwd(&pwd_raw);
    if include_cwd {
        if !paths.iter().any(|entry| entry.is_empty()) {
            paths.insert(0, String::new());
        }
    }

    SysBootstrapState {
        path: paths,
        stdlib_root,
        pythonpath_entries,
        module_roots_entries,
        venv_site_packages_entries,
        py_path_raw,
        module_roots_raw,
        virtual_env_raw,
        dev_trusted_raw,
        pwd,
        include_cwd,
    }
}

struct SourceLoaderResolution {
    is_package: bool,
    module_package: String,
    package_root: Option<String>,
}

struct ImportlibSourceExecPayload {
    source: Vec<u8>,
    is_package: bool,
    module_package: String,
    package_root: Option<String>,
}

struct ImportlibPathResolution {
    origin: String,
    is_package: bool,
    submodule_search_locations: Option<Vec<String>>,
    cached: String,
}

struct ImportlibFindSpecPayload {
    origin: Option<String>,
    is_package: bool,
    submodule_search_locations: Option<Vec<String>>,
    cached: Option<String>,
    is_builtin: bool,
    has_location: bool,
    loader_kind: String,
    meta_path_count: i64,
    path_hooks_count: i64,
}

struct ImportlibSpecFromFileLocationPayload {
    path: String,
    is_package: bool,
    package_root: Option<String>,
}

struct ImportlibBootstrapPayload {
    resolved_search_paths: Vec<String>,
    pythonpath_entries: Vec<String>,
    module_roots_entries: Vec<String>,
    venv_site_packages_entries: Vec<String>,
    pwd: String,
    include_cwd: bool,
    stdlib_root: Option<String>,
}

struct ImportlibResourcesPathPayload {
    basename: String,
    exists: bool,
    is_file: bool,
    is_dir: bool,
    entries: Vec<String>,
    has_init_py: bool,
}

struct ImportlibResourcesPackagePayload {
    roots: Vec<String>,
    is_namespace: bool,
    has_regular_package: bool,
    init_file: Option<String>,
}

struct ImportlibMetadataPayload {
    path: String,
    name: String,
    version: String,
    metadata: Vec<(String, String)>,
    entry_points: Vec<(String, String, String)>,
    requires_dist: Vec<String>,
    provides_extra: Vec<String>,
    requires_python: Option<String>,
}

fn bootstrap_path_sep() -> char {
    if sys_platform_str().starts_with("win") {
        '\\'
    } else {
        '/'
    }
}

fn path_is_absolute_text(path: &str, sep: char) -> bool {
    if path.starts_with(sep) {
        return true;
    }
    if sep == '\\' {
        let bytes = path.as_bytes();
        if bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes[2] == b'\\' || bytes[2] == b'/')
        {
            return true;
        }
        if path.starts_with("\\\\") {
            return true;
        }
    }
    false
}

fn source_loader_resolution(
    module_name: &str,
    path: &str,
    spec_is_package: bool,
) -> SourceLoaderResolution {
    let sep = bootstrap_path_sep();
    let is_init = path_basename_text(path, sep) == "__init__.py";
    let is_package = spec_is_package || is_init;
    let module_package = if is_package {
        module_name.to_string()
    } else {
        module_name
            .rsplit_once('.')
            .map(|(prefix, _)| prefix.to_string())
            .unwrap_or_default()
    };
    let package_root = if is_package {
        Some(path_dirname_text(path, sep))
    } else {
        None
    };
    SourceLoaderResolution {
        is_package,
        module_package,
        package_root,
    }
}

fn importlib_source_exec_payload(
    module_name: &str,
    path: &str,
    spec_is_package: bool,
) -> Result<ImportlibSourceExecPayload, std::io::Error> {
    let source_bytes = std::fs::read(path)?;
    let source =
        match crate::object::ops::decode_bytes_text("utf-8", "surrogateescape", &source_bytes) {
            Ok((text, _encoding)) => text,
            Err(_) => String::from_utf8_lossy(&source_bytes)
                .into_owned()
                .into_bytes(),
        };
    let resolution = source_loader_resolution(module_name, path, spec_is_package);
    Ok(ImportlibSourceExecPayload {
        source,
        is_package: resolution.is_package,
        module_package: resolution.module_package,
        package_root: resolution.package_root,
    })
}

fn importlib_cache_from_source(path: &str) -> String {
    let sep = bootstrap_path_sep();
    let base = path_basename_text(path, sep);
    if base.ends_with(".py") {
        let cache_dir = path_join_text(path_dirname_text(path, sep), "__pycache__", sep);
        return path_join_text(cache_dir, &format!("{base}c"), sep);
    }
    format!("{path}c")
}

fn importlib_find_in_path(
    fullname: &str,
    search_paths: &[String],
) -> Option<ImportlibPathResolution> {
    let sep = bootstrap_path_sep();
    let parts: Vec<&str> = fullname.split('.').collect();
    if parts.is_empty() {
        return None;
    }
    let mut current_paths = search_paths.to_vec();
    for (idx, part) in parts.iter().enumerate() {
        let is_last = idx + 1 == parts.len();
        let mut found_pkg = false;
        let mut next_paths: Vec<String> = Vec::new();
        for base in &current_paths {
            let root = if base.is_empty() {
                ".".to_string()
            } else {
                base.clone()
            };
            let pkg_dir = path_join_text(root.clone(), part, sep);
            let init_file = path_join_text(pkg_dir.clone(), "__init__.py", sep);
            if std::fs::metadata(&init_file)
                .map(|metadata| metadata.is_file())
                .unwrap_or(false)
            {
                if is_last {
                    return Some(ImportlibPathResolution {
                        origin: init_file.clone(),
                        is_package: true,
                        submodule_search_locations: Some(vec![pkg_dir]),
                        cached: importlib_cache_from_source(&init_file),
                    });
                }
                next_paths = vec![pkg_dir];
                found_pkg = true;
                break;
            }
            if is_last {
                let mod_file = path_join_text(root, &format!("{part}.py"), sep);
                if std::fs::metadata(&mod_file)
                    .map(|metadata| metadata.is_file())
                    .unwrap_or(false)
                {
                    return Some(ImportlibPathResolution {
                        origin: mod_file.clone(),
                        is_package: false,
                        submodule_search_locations: None,
                        cached: importlib_cache_from_source(&mod_file),
                    });
                }
            }
        }
        if found_pkg {
            current_paths = next_paths;
            continue;
        }
        return None;
    }
    None
}

fn importlib_search_paths(search_paths: &[String], module_file: Option<String>) -> Vec<String> {
    let sep = bootstrap_path_sep();
    let state = sys_bootstrap_state_from_module_file(module_file);
    let mut out: Vec<String> = Vec::new();
    for entry in search_paths {
        append_unique_path(&mut out, entry);
    }
    if let Some(stdlib_root) = state.stdlib_root.as_deref() {
        append_unique_path(&mut out, stdlib_root);
    }
    for root in &state.module_roots_entries {
        append_unique_path(&mut out, root);
    }
    for root in &state.venv_site_packages_entries {
        append_unique_path(&mut out, root);
    }
    for base in search_paths {
        let root = if base.is_empty() { "." } else { base.as_str() };
        let candidate =
            path_join_text(path_join_text(root.to_string(), "molt", sep), "stdlib", sep);
        append_unique_path(&mut out, &candidate);
    }
    out
}

fn importlib_bootstrap_payload(
    search_paths: &[String],
    module_file: Option<String>,
) -> ImportlibBootstrapPayload {
    let state = sys_bootstrap_state_from_module_file(module_file.clone());
    let resolved_search_paths = importlib_search_paths(search_paths, module_file);
    ImportlibBootstrapPayload {
        resolved_search_paths,
        pythonpath_entries: state.pythonpath_entries,
        module_roots_entries: state.module_roots_entries,
        venv_site_packages_entries: state.venv_site_packages_entries,
        pwd: state.pwd,
        include_cwd: state.include_cwd,
        stdlib_root: state.stdlib_root,
    }
}

fn importlib_namespace_paths(
    package: &str,
    search_paths: &[String],
    module_file: Option<String>,
) -> Vec<String> {
    if package.is_empty() {
        return Vec::new();
    }
    let sep = bootstrap_path_sep();
    let mut resolved = importlib_search_paths(search_paths, module_file.clone());
    if !resolved.iter().any(|entry| entry.is_empty()) {
        resolved.push(String::new());
    }
    let state = sys_bootstrap_state_from_module_file(module_file);
    if !state.pwd.is_empty() {
        append_unique_path(&mut resolved, &state.pwd);
    }
    let mut matches: Vec<String> = Vec::new();
    let parts: Vec<&str> = package.split('.').filter(|part| !part.is_empty()).collect();
    if parts.is_empty() {
        return matches;
    }
    for base in resolved {
        let mut path = if base.is_empty() {
            ".".to_string()
        } else {
            base
        };
        for part in &parts {
            path = path_join_text(path, part, sep);
        }
        if std::fs::metadata(&path)
            .map(|metadata| metadata.is_dir())
            .unwrap_or(false)
        {
            append_unique_path(&mut matches, &path);
        }
    }
    matches
}

fn importlib_metadata_dist_paths(
    search_paths: &[String],
    module_file: Option<String>,
) -> Vec<String> {
    let resolved = importlib_search_paths(search_paths, module_file);
    let mut out: Vec<String> = Vec::new();
    for base in resolved {
        if base.is_empty() {
            continue;
        }
        let read_dir = match std::fs::read_dir(&base) {
            Ok(entries) => entries,
            Err(_) => continue,
        };
        for entry in read_dir.flatten() {
            let file_type = match entry.file_type() {
                Ok(kind) => kind,
                Err(_) => continue,
            };
            if !file_type.is_dir() {
                continue;
            }
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if !name.ends_with(".dist-info") && !name.ends_with(".egg-info") {
                continue;
            }
            let path = entry.path();
            let path_text = path.to_string_lossy().into_owned();
            append_unique_path(&mut out, &path_text);
        }
    }
    out
}

fn importlib_metadata_entry_points_payload(
    search_paths: &[String],
    module_file: Option<String>,
) -> Vec<(String, String, String)> {
    importlib_metadata_entry_points_select_payload(search_paths, module_file, None, None)
}

fn importlib_metadata_entry_points_select_payload(
    search_paths: &[String],
    module_file: Option<String>,
    group: Option<&str>,
    name: Option<&str>,
) -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = Vec::new();
    let dist_paths = importlib_metadata_dist_paths(search_paths, module_file);
    for path in dist_paths {
        let payload = importlib_metadata_payload(&path);
        for (entry_name, entry_value, entry_group) in payload.entry_points {
            if let Some(expected_group) = group {
                if entry_group != expected_group {
                    continue;
                }
            }
            if let Some(expected_name) = name {
                if entry_name != expected_name {
                    continue;
                }
            }
            out.push((entry_name, entry_value, entry_group));
        }
    }
    out
}

fn importlib_metadata_normalize_name(name: &str) -> String {
    let mut out = String::with_capacity(name.len());
    let mut prev_sep = false;
    for ch in name.chars() {
        if matches!(ch, '-' | '_' | '.') {
            if !prev_sep {
                out.push('-');
                prev_sep = true;
            }
            continue;
        }
        for lowered in ch.to_lowercase() {
            out.push(lowered);
        }
        prev_sep = false;
    }
    out
}

fn importlib_find_spec_payload(
    fullname: &str,
    search_paths: &[String],
    module_file: Option<String>,
    meta_path_count: i64,
    path_hooks_count: i64,
) -> Option<ImportlibFindSpecPayload> {
    if fullname == "math" {
        return Some(ImportlibFindSpecPayload {
            origin: Some("built-in".to_string()),
            is_package: false,
            submodule_search_locations: None,
            cached: None,
            is_builtin: true,
            has_location: false,
            loader_kind: "builtin".to_string(),
            meta_path_count,
            path_hooks_count,
        });
    }
    let resolved = importlib_search_paths(search_paths, module_file);
    let resolution = importlib_find_in_path(fullname, &resolved)?;
    Some(ImportlibFindSpecPayload {
        origin: Some(resolution.origin),
        is_package: resolution.is_package,
        submodule_search_locations: resolution.submodule_search_locations,
        cached: Some(resolution.cached),
        is_builtin: false,
        has_location: true,
        loader_kind: "source".to_string(),
        meta_path_count,
        path_hooks_count,
    })
}

fn importlib_resources_path_payload(path: &str) -> ImportlibResourcesPathPayload {
    let sep = bootstrap_path_sep();
    let basename = path_basename_text(path, sep);
    let mut entries: Vec<String> = Vec::new();
    let mut has_init_py = false;
    let mut exists = false;
    let mut is_file = false;
    let mut is_dir = false;
    if let Ok(metadata) = std::fs::metadata(path) {
        exists = true;
        is_file = metadata.is_file();
        is_dir = metadata.is_dir();
    }
    if is_dir {
        if let Ok(read_dir) = std::fs::read_dir(path) {
            for entry in read_dir.flatten() {
                let name = entry.file_name().to_string_lossy().into_owned();
                if name == "__init__.py" {
                    has_init_py = true;
                }
                entries.push(name);
            }
            entries.sort();
        }
    }
    ImportlibResourcesPathPayload {
        basename,
        exists,
        is_file,
        is_dir,
        entries,
        has_init_py,
    }
}

fn importlib_resources_package_payload(
    package: &str,
    search_paths: &[String],
    module_file: Option<String>,
) -> ImportlibResourcesPackagePayload {
    let resolved = importlib_search_paths(search_paths, module_file.clone());
    let resolution = importlib_find_in_path(package, &resolved);
    let mut roots: Vec<String> = Vec::new();
    let mut has_regular_package = false;
    let mut init_file: Option<String> = None;
    if let Some(spec) = resolution {
        if spec.is_package {
            has_regular_package = true;
            init_file = Some(spec.origin.clone());
            if let Some(locations) = spec.submodule_search_locations {
                for location in locations {
                    append_unique_path(&mut roots, &location);
                }
            } else {
                let sep = bootstrap_path_sep();
                let dir = path_dirname_text(&spec.origin, sep);
                if !dir.is_empty() {
                    append_unique_path(&mut roots, &dir);
                }
            }
        }
    }
    let namespace_roots = importlib_namespace_paths(package, search_paths, module_file);
    for root in &namespace_roots {
        append_unique_path(&mut roots, root);
    }
    let is_namespace = !has_regular_package && !namespace_roots.is_empty();
    ImportlibResourcesPackagePayload {
        roots,
        is_namespace,
        has_regular_package,
        init_file,
    }
}

fn importlib_metadata_parse_headers(text: &str) -> Vec<(String, String)> {
    let mut mapping: Vec<(String, String)> = Vec::new();
    let mut current_idx: Option<usize> = None;
    for raw_line in text.lines() {
        if raw_line.is_empty() {
            current_idx = None;
            continue;
        }
        let bytes = raw_line.as_bytes();
        if !bytes.is_empty() && (bytes[0] == b' ' || bytes[0] == b'\t') {
            if let Some(idx) = current_idx {
                mapping[idx].1.push('\n');
                mapping[idx].1.push_str(raw_line.trim());
            }
            continue;
        }
        if let Some((key, value)) = raw_line.split_once(':') {
            mapping.push((key.trim().to_string(), value.trim().to_string()));
            current_idx = Some(mapping.len() - 1);
        }
    }
    mapping
}

fn importlib_metadata_header_values(headers: &[(String, String)], key: &str) -> Vec<String> {
    headers
        .iter()
        .filter_map(|(k, v)| {
            if k.eq_ignore_ascii_case(key) {
                Some(v.clone())
            } else {
                None
            }
        })
        .collect()
}

fn importlib_metadata_first_nonempty(headers: &[(String, String)], key: &str) -> Option<String> {
    importlib_metadata_header_values(headers, key)
        .into_iter()
        .find(|value| !value.is_empty())
}

fn importlib_metadata_parse_entry_points(text: &str) -> Vec<(String, String, String)> {
    let mut group: Option<String> = None;
    let mut out: Vec<(String, String, String)> = Vec::new();
    for line in text.lines() {
        let stripped = line.trim();
        if stripped.is_empty() || stripped.starts_with('#') {
            continue;
        }
        if stripped.starts_with('[') && stripped.ends_with(']') {
            group = Some(stripped[1..stripped.len() - 1].trim().to_string());
            continue;
        }
        let Some(current_group) = group.as_ref() else {
            continue;
        };
        let Some((name, value)) = stripped.split_once('=') else {
            continue;
        };
        out.push((
            name.trim().to_string(),
            value.trim().to_string(),
            current_group.clone(),
        ));
    }
    out
}

fn importlib_metadata_payload(path: &str) -> ImportlibMetadataPayload {
    let sep = bootstrap_path_sep();
    let base = path_basename_text(path, sep);
    let fallback_name = base
        .split_once('-')
        .map(|(name, _)| name)
        .unwrap_or(base.as_str())
        .to_string();

    let metadata_path = path_join_text(path.to_string(), "METADATA", sep);
    let pkg_info_path = path_join_text(path.to_string(), "PKG-INFO", sep);
    let metadata_text = std::fs::read(&metadata_path)
        .or_else(|_| std::fs::read(&pkg_info_path))
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned());
    let metadata_pairs = metadata_text
        .as_deref()
        .map(importlib_metadata_parse_headers)
        .unwrap_or_default();
    let name = importlib_metadata_first_nonempty(&metadata_pairs, "Name").unwrap_or(fallback_name);
    let version = importlib_metadata_first_nonempty(&metadata_pairs, "Version").unwrap_or_default();
    let requires_python = importlib_metadata_first_nonempty(&metadata_pairs, "Requires-Python");
    let requires_dist = importlib_metadata_header_values(&metadata_pairs, "Requires-Dist");
    let provides_extra = importlib_metadata_header_values(&metadata_pairs, "Provides-Extra");

    let entry_points_path = path_join_text(path.to_string(), "entry_points.txt", sep);
    let entry_points = std::fs::read(&entry_points_path)
        .ok()
        .map(|bytes| String::from_utf8_lossy(&bytes).into_owned())
        .as_deref()
        .map(importlib_metadata_parse_entry_points)
        .unwrap_or_default();

    ImportlibMetadataPayload {
        path: path.to_string(),
        name,
        version,
        metadata: metadata_pairs,
        entry_points,
        requires_dist,
        provides_extra,
        requires_python,
    }
}

fn importlib_spec_from_file_location_payload(path: &str) -> ImportlibSpecFromFileLocationPayload {
    let sep = bootstrap_path_sep();
    let is_package = path_basename_text(path, sep) == "__init__.py";
    let package_root = if is_package {
        Some(path_dirname_text(path, sep))
    } else {
        None
    };
    ImportlibSpecFromFileLocationPayload {
        path: path.to_string(),
        is_package,
        package_root,
    }
}

fn raise_importlib_io_error(_py: &PyToken<'_>, err: std::io::Error) -> u64 {
    let message = err.to_string();
    match err.kind() {
        std::io::ErrorKind::NotFound => {
            raise_exception::<u64>(_py, "FileNotFoundError", message.as_str())
        }
        std::io::ErrorKind::PermissionDenied => {
            raise_exception::<u64>(_py, "PermissionError", message.as_str())
        }
        std::io::ErrorKind::IsADirectory => {
            raise_exception::<u64>(_py, "IsADirectoryError", message.as_str())
        }
        _ => raise_exception::<u64>(_py, "OSError", message.as_str()),
    }
}

fn bootstrap_resolve_abspath(path: &str, module_file: Option<String>) -> String {
    let sep = bootstrap_path_sep();
    let state = sys_bootstrap_state_from_module_file(module_file);
    let joined = if path_is_absolute_text(path, sep) {
        path.to_string()
    } else if state.pwd.is_empty() {
        path.to_string()
    } else {
        path_join_text(state.pwd, path, sep)
    };
    path_normpath_text(&joined, sep)
}

fn module_file_from_bits(_py: &PyToken<'_>, module_file_bits: u64) -> Result<Option<String>, u64> {
    if obj_from_bits(module_file_bits).is_none() {
        return Ok(None);
    }
    match string_obj_to_owned(obj_from_bits(module_file_bits)) {
        Some(value) => Ok(Some(value)),
        None => Err(raise_exception::<_>(
            _py,
            "TypeError",
            "module file must be str or None",
        )),
    }
}

fn alloc_string_list_bits(_py: &PyToken<'_>, values: &[String]) -> Option<u64> {
    let mut bits_vec: Vec<u64> = Vec::with_capacity(values.len());
    for value in values {
        let ptr = alloc_string(_py, value.as_bytes());
        if ptr.is_null() {
            for bits in bits_vec {
                dec_ref_bits(_py, bits);
            }
            return None;
        }
        bits_vec.push(MoltObject::from_ptr(ptr).bits());
    }
    let list_ptr = alloc_list(_py, bits_vec.as_slice());
    for bits in bits_vec {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        return None;
    }
    Some(MoltObject::from_ptr(list_ptr).bits())
}

fn alloc_string_pairs_dict_bits(_py: &PyToken<'_>, values: &[(String, String)]) -> Option<u64> {
    let mut pairs: Vec<u64> = Vec::with_capacity(values.len() * 2);
    let mut owned: Vec<u64> = Vec::with_capacity(values.len() * 2);
    for (key, value) in values {
        let key_ptr = alloc_string(_py, key.as_bytes());
        if key_ptr.is_null() {
            for bits in owned {
                dec_ref_bits(_py, bits);
            }
            return None;
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let value_ptr = alloc_string(_py, value.as_bytes());
        if value_ptr.is_null() {
            dec_ref_bits(_py, key_bits);
            for bits in owned {
                dec_ref_bits(_py, bits);
            }
            return None;
        }
        let value_bits = MoltObject::from_ptr(value_ptr).bits();
        pairs.push(key_bits);
        pairs.push(value_bits);
        owned.push(key_bits);
        owned.push(value_bits);
    }
    let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
    for bits in owned {
        dec_ref_bits(_py, bits);
    }
    if dict_ptr.is_null() {
        None
    } else {
        Some(MoltObject::from_ptr(dict_ptr).bits())
    }
}

fn alloc_string_triplets_list_bits(
    _py: &PyToken<'_>,
    values: &[(String, String, String)],
) -> Option<u64> {
    let mut tuple_bits_vec: Vec<u64> = Vec::with_capacity(values.len());
    for (name, value, group) in values {
        let name_ptr = alloc_string(_py, name.as_bytes());
        if name_ptr.is_null() {
            for bits in tuple_bits_vec {
                dec_ref_bits(_py, bits);
            }
            return None;
        }
        let name_bits = MoltObject::from_ptr(name_ptr).bits();
        let value_ptr = alloc_string(_py, value.as_bytes());
        if value_ptr.is_null() {
            dec_ref_bits(_py, name_bits);
            for bits in tuple_bits_vec {
                dec_ref_bits(_py, bits);
            }
            return None;
        }
        let value_bits = MoltObject::from_ptr(value_ptr).bits();
        let group_ptr = alloc_string(_py, group.as_bytes());
        if group_ptr.is_null() {
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, value_bits);
            for bits in tuple_bits_vec {
                dec_ref_bits(_py, bits);
            }
            return None;
        }
        let group_bits = MoltObject::from_ptr(group_ptr).bits();
        let tuple_ptr = alloc_tuple(_py, &[name_bits, value_bits, group_bits]);
        dec_ref_bits(_py, name_bits);
        dec_ref_bits(_py, value_bits);
        dec_ref_bits(_py, group_bits);
        if tuple_ptr.is_null() {
            for bits in tuple_bits_vec {
                dec_ref_bits(_py, bits);
            }
            return None;
        }
        tuple_bits_vec.push(MoltObject::from_ptr(tuple_ptr).bits());
    }
    let list_ptr = alloc_list(_py, tuple_bits_vec.as_slice());
    for bits in tuple_bits_vec {
        dec_ref_bits(_py, bits);
    }
    if list_ptr.is_null() {
        None
    } else {
        Some(MoltObject::from_ptr(list_ptr).bits())
    }
}

fn locale_encoding_label(locale: &str) -> &'static str {
    if locale == "C" || locale == "POSIX" {
        "US-ASCII"
    } else {
        "UTF-8"
    }
}

fn alloc_str_bits(_py: &PyToken<'_>, value: &str) -> Result<u64, u64> {
    let ptr = alloc_string(_py, value.as_bytes());
    if ptr.is_null() {
        return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
    }
    Ok(MoltObject::from_ptr(ptr).bits())
}

fn string_arg_from_bits(_py: &PyToken<'_>, bits: u64, name: &str) -> Result<String, u64> {
    match string_obj_to_owned(obj_from_bits(bits)) {
        Some(value) => Ok(value),
        None => Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{name} must be str"),
        )),
    }
}

fn optional_string_arg_from_bits(
    _py: &PyToken<'_>,
    bits: u64,
    name: &str,
) -> Result<Option<String>, u64> {
    if obj_from_bits(bits).is_none() {
        return Ok(None);
    }
    match string_obj_to_owned(obj_from_bits(bits)) {
        Some(value) => Ok(Some(value)),
        None => Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{name} must be str or None"),
        )),
    }
}

fn string_sequence_arg_from_bits(
    _py: &PyToken<'_>,
    bits: u64,
    name: &str,
) -> Result<Vec<String>, u64> {
    let iter_bits = molt_iter(bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let mut out: Vec<String> = Vec::new();
    loop {
        let pair_bits = molt_iter_next(iter_bits);
        let Some(pair_ptr) = maybe_ptr_from_bits(pair_bits) else {
            return Err(MoltObject::none().bits());
        };
        unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return Err(MoltObject::none().bits());
            }
        }
        let pair = unsafe { seq_vec_ref(pair_ptr) };
        if pair.len() < 2 {
            return Err(MoltObject::none().bits());
        }
        if is_truthy(_py, obj_from_bits(pair[1])) {
            break;
        }
        let Some(text) = string_obj_to_owned(obj_from_bits(pair[0])) else {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                &format!("{name} entries must be str"),
            ));
        };
        out.push(text);
    }
    Ok(out)
}

fn iterable_count_arg_from_bits(_py: &PyToken<'_>, bits: u64, name: &str) -> Result<i64, u64> {
    if obj_from_bits(bits).is_none() {
        return Ok(0);
    }
    let iter_bits = molt_iter(bits);
    if exception_pending(_py) {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{name} must be iterable"),
        ));
    }
    let mut count: i64 = 0;
    loop {
        let pair_bits = molt_iter_next(iter_bits);
        let Some(pair_ptr) = maybe_ptr_from_bits(pair_bits) else {
            return Err(MoltObject::none().bits());
        };
        unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                return Err(MoltObject::none().bits());
            }
        }
        let pair = unsafe { seq_vec_ref(pair_ptr) };
        if pair.len() < 2 {
            return Err(MoltObject::none().bits());
        }
        if is_truthy(_py, obj_from_bits(pair[1])) {
            break;
        }
        count += 1;
    }
    Ok(count)
}

#[no_mangle]
pub extern "C" fn molt_sys_bootstrap_path(module_file_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let state = sys_bootstrap_state_from_module_file(module_file);
        alloc_string_list_bits(_py, &state.path).unwrap_or_else(|| MoltObject::none().bits())
    })
}

#[no_mangle]
pub extern "C" fn molt_sys_bootstrap_pythonpath(module_file_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let state = sys_bootstrap_state_from_module_file(module_file);
        alloc_string_list_bits(_py, &state.pythonpath_entries)
            .unwrap_or_else(|| MoltObject::none().bits())
    })
}

#[no_mangle]
pub extern "C" fn molt_sys_bootstrap_module_roots(module_file_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let state = sys_bootstrap_state_from_module_file(module_file);
        alloc_string_list_bits(_py, &state.module_roots_entries)
            .unwrap_or_else(|| MoltObject::none().bits())
    })
}

#[no_mangle]
pub extern "C" fn molt_sys_bootstrap_pwd(module_file_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let state = sys_bootstrap_state_from_module_file(module_file);
        match alloc_str_bits(_py, &state.pwd) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_sys_bootstrap_include_cwd(module_file_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let state = sys_bootstrap_state_from_module_file(module_file);
        MoltObject::from_bool(state.include_cwd).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_sys_bootstrap_stdlib_root(module_file_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let state = sys_bootstrap_state_from_module_file(module_file);
        match state.stdlib_root {
            Some(root) => match alloc_str_bits(_py, &root) {
                Ok(bits) => bits,
                Err(err) => err,
            },
            None => MoltObject::none().bits(),
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_sys_bootstrap_payload(module_file_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let state = sys_bootstrap_state_from_module_file(module_file);
        let path_bits = match alloc_string_list_bits(_py, &state.path) {
            Some(bits) => bits,
            None => return MoltObject::none().bits(),
        };
        let pythonpath_entries_bits = match alloc_string_list_bits(_py, &state.pythonpath_entries) {
            Some(bits) => bits,
            None => return MoltObject::none().bits(),
        };
        let module_roots_entries_bits =
            match alloc_string_list_bits(_py, &state.module_roots_entries) {
                Some(bits) => bits,
                None => return MoltObject::none().bits(),
            };
        let venv_site_packages_entries_bits =
            match alloc_string_list_bits(_py, &state.venv_site_packages_entries) {
                Some(bits) => bits,
                None => return MoltObject::none().bits(),
            };
        let pythonpath_bits = match alloc_str_bits(_py, &state.py_path_raw) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let module_roots_bits = match alloc_str_bits(_py, &state.module_roots_raw) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let virtual_env_bits = match alloc_str_bits(_py, &state.virtual_env_raw) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let dev_trusted_bits = match alloc_str_bits(_py, &state.dev_trusted_raw) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let pwd_bits = match alloc_str_bits(_py, &state.pwd) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let stdlib_root_bits = match state.stdlib_root {
            Some(root) => match alloc_str_bits(_py, &root) {
                Ok(bits) => bits,
                Err(err) => return err,
            },
            None => MoltObject::none().bits(),
        };
        let include_cwd_bits = MoltObject::from_bool(state.include_cwd).bits();

        let keys_and_values: [(&[u8], u64); 11] = [
            (b"path", path_bits),
            (b"pythonpath_entries", pythonpath_entries_bits),
            (b"module_roots_entries", module_roots_entries_bits),
            (
                b"venv_site_packages_entries",
                venv_site_packages_entries_bits,
            ),
            (b"pythonpath", pythonpath_bits),
            (b"module_roots", module_roots_bits),
            (b"virtual_env", virtual_env_bits),
            (b"dev_trusted", dev_trusted_bits),
            (b"pwd", pwd_bits),
            (b"stdlib_root", stdlib_root_bits),
            (b"include_cwd", include_cwd_bits),
        ];
        let mut pairs: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        let mut owned: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        for (key, value_bits) in keys_and_values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
            owned.push(value_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(dict_ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_importlib_source_loader_payload(
    module_name_bits: u64,
    path_bits: u64,
    spec_is_package_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let spec_is_package = is_truthy(_py, obj_from_bits(spec_is_package_bits));
        let resolution = source_loader_resolution(&module_name, &path, spec_is_package);

        let module_package_bits = match alloc_str_bits(_py, &resolution.module_package) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let package_root_bits = match resolution.package_root.as_deref() {
            Some(root) => match alloc_str_bits(_py, root) {
                Ok(bits) => bits,
                Err(err) => {
                    dec_ref_bits(_py, module_package_bits);
                    return err;
                }
            },
            None => MoltObject::none().bits(),
        };
        let is_package_bits = MoltObject::from_bool(resolution.is_package).bits();

        let keys_and_values: [(&[u8], u64); 3] = [
            (b"is_package", is_package_bits),
            (b"module_package", module_package_bits),
            (b"package_root", package_root_bits),
        ];
        let mut pairs: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        let mut owned: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        for (key, value_bits) in keys_and_values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
            owned.push(value_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(dict_ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_importlib_source_exec_payload(
    module_name_bits: u64,
    path_bits: u64,
    spec_is_package_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let spec_is_package = is_truthy(_py, obj_from_bits(spec_is_package_bits));
        let payload = match importlib_source_exec_payload(&module_name, &path, spec_is_package) {
            Ok(value) => value,
            Err(err) => return raise_importlib_io_error(_py, err),
        };

        let source_ptr = alloc_string(_py, &payload.source);
        if source_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        let source_bits = MoltObject::from_ptr(source_ptr).bits();
        let module_package_bits = match alloc_str_bits(_py, &payload.module_package) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, source_bits);
                return err;
            }
        };
        let package_root_bits = match payload.package_root.as_deref() {
            Some(root) => match alloc_str_bits(_py, root) {
                Ok(bits) => bits,
                Err(err) => {
                    dec_ref_bits(_py, source_bits);
                    dec_ref_bits(_py, module_package_bits);
                    return err;
                }
            },
            None => MoltObject::none().bits(),
        };
        let is_package_bits = MoltObject::from_bool(payload.is_package).bits();

        let keys_and_values: [(&[u8], u64); 4] = [
            (b"source", source_bits),
            (b"is_package", is_package_bits),
            (b"module_package", module_package_bits),
            (b"package_root", package_root_bits),
        ];
        let mut pairs: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        let mut owned: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        for (key, value_bits) in keys_and_values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
            owned.push(value_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(dict_ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_importlib_read_file(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(err) => return raise_importlib_io_error(_py, err),
        };
        let out_ptr = alloc_bytes(_py, &bytes);
        if out_ptr.is_null() {
            raise_exception::<_>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_importlib_cache_from_source(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let cached = importlib_cache_from_source(&path);
        match alloc_str_bits(_py, &cached) {
            Ok(bits) => bits,
            Err(err_bits) => err_bits,
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_importlib_find_in_path(fullname_bits: u64, search_paths_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let fullname = match string_arg_from_bits(_py, fullname_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let search_paths =
            match string_sequence_arg_from_bits(_py, search_paths_bits, "search paths") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let Some(resolution) = importlib_find_in_path(&fullname, &search_paths) else {
            return MoltObject::none().bits();
        };
        let origin_bits = match alloc_str_bits(_py, &resolution.origin) {
            Ok(bits) => bits,
            Err(err_bits) => return err_bits,
        };
        let locations_bits = match resolution.submodule_search_locations.as_ref() {
            Some(entries) => match alloc_string_list_bits(_py, entries) {
                Some(bits) => bits,
                None => {
                    dec_ref_bits(_py, origin_bits);
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
            },
            None => MoltObject::none().bits(),
        };
        let cached_bits = match alloc_str_bits(_py, &resolution.cached) {
            Ok(bits) => bits,
            Err(err_bits) => {
                dec_ref_bits(_py, origin_bits);
                if !obj_from_bits(locations_bits).is_none() {
                    dec_ref_bits(_py, locations_bits);
                }
                return err_bits;
            }
        };
        let is_package_bits = MoltObject::from_bool(resolution.is_package).bits();
        let keys_and_values: [(&[u8], u64); 4] = [
            (b"origin", origin_bits),
            (b"is_package", is_package_bits),
            (b"submodule_search_locations", locations_bits),
            (b"cached", cached_bits),
        ];
        let mut pairs: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        let mut owned: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        for (key, value_bits) in keys_and_values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
            owned.push(value_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(dict_ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_importlib_find_spec_payload(
    fullname_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
    meta_path_bits: u64,
    path_hooks_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let fullname = match string_arg_from_bits(_py, fullname_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let search_paths =
            match string_sequence_arg_from_bits(_py, search_paths_bits, "search paths") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let meta_path_count = match iterable_count_arg_from_bits(_py, meta_path_bits, "meta_path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let path_hooks_count =
            match iterable_count_arg_from_bits(_py, path_hooks_bits, "path_hooks") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        if fullname != "math" && !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let Some(payload) = importlib_find_spec_payload(
            &fullname,
            &search_paths,
            module_file,
            meta_path_count,
            path_hooks_count,
        ) else {
            return MoltObject::none().bits();
        };
        let origin_bits = match payload.origin.as_deref() {
            Some(origin) => match alloc_str_bits(_py, origin) {
                Ok(bits) => bits,
                Err(err) => return err,
            },
            None => MoltObject::none().bits(),
        };
        let locations_bits = match payload.submodule_search_locations.as_ref() {
            Some(entries) => match alloc_string_list_bits(_py, entries) {
                Some(bits) => bits,
                None => {
                    if !obj_from_bits(origin_bits).is_none() {
                        dec_ref_bits(_py, origin_bits);
                    }
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
            },
            None => MoltObject::none().bits(),
        };
        let cached_bits = match payload.cached.as_deref() {
            Some(cached) => match alloc_str_bits(_py, cached) {
                Ok(bits) => bits,
                Err(err) => {
                    if !obj_from_bits(origin_bits).is_none() {
                        dec_ref_bits(_py, origin_bits);
                    }
                    if !obj_from_bits(locations_bits).is_none() {
                        dec_ref_bits(_py, locations_bits);
                    }
                    return err;
                }
            },
            None => MoltObject::none().bits(),
        };
        let loader_kind_bits = match alloc_str_bits(_py, &payload.loader_kind) {
            Ok(bits) => bits,
            Err(err) => {
                if !obj_from_bits(origin_bits).is_none() {
                    dec_ref_bits(_py, origin_bits);
                }
                if !obj_from_bits(locations_bits).is_none() {
                    dec_ref_bits(_py, locations_bits);
                }
                if !obj_from_bits(cached_bits).is_none() {
                    dec_ref_bits(_py, cached_bits);
                }
                return err;
            }
        };
        let is_package_bits = MoltObject::from_bool(payload.is_package).bits();
        let is_builtin_bits = MoltObject::from_bool(payload.is_builtin).bits();
        let has_location_bits = MoltObject::from_bool(payload.has_location).bits();
        let meta_path_count_bits = int_bits_from_i64(_py, payload.meta_path_count);
        let path_hooks_count_bits = int_bits_from_i64(_py, payload.path_hooks_count);
        let keys_and_values: [(&[u8], u64); 9] = [
            (b"origin", origin_bits),
            (b"is_package", is_package_bits),
            (b"submodule_search_locations", locations_bits),
            (b"cached", cached_bits),
            (b"is_builtin", is_builtin_bits),
            (b"has_location", has_location_bits),
            (b"loader_kind", loader_kind_bits),
            (b"meta_path_count", meta_path_count_bits),
            (b"path_hooks_count", path_hooks_count_bits),
        ];
        let mut pairs: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        let mut owned: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        for (key, value_bits) in keys_and_values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
            owned.push(value_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(dict_ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_importlib_bootstrap_payload(
    search_paths_bits: u64,
    module_file_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let search_paths =
            match string_sequence_arg_from_bits(_py, search_paths_bits, "search paths") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let payload = importlib_bootstrap_payload(&search_paths, module_file);
        let resolved_paths_bits = match alloc_string_list_bits(_py, &payload.resolved_search_paths)
        {
            Some(bits) => bits,
            None => return raise_exception::<_>(_py, "MemoryError", "out of memory"),
        };
        let pythonpath_bits = match alloc_string_list_bits(_py, &payload.pythonpath_entries) {
            Some(bits) => bits,
            None => {
                dec_ref_bits(_py, resolved_paths_bits);
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
        };
        let module_roots_bits = match alloc_string_list_bits(_py, &payload.module_roots_entries) {
            Some(bits) => bits,
            None => {
                dec_ref_bits(_py, resolved_paths_bits);
                dec_ref_bits(_py, pythonpath_bits);
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
        };
        let venv_site_packages_bits =
            match alloc_string_list_bits(_py, &payload.venv_site_packages_entries) {
                Some(bits) => bits,
                None => {
                    dec_ref_bits(_py, resolved_paths_bits);
                    dec_ref_bits(_py, pythonpath_bits);
                    dec_ref_bits(_py, module_roots_bits);
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
            };
        let pwd_bits = match alloc_str_bits(_py, &payload.pwd) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, resolved_paths_bits);
                dec_ref_bits(_py, pythonpath_bits);
                dec_ref_bits(_py, module_roots_bits);
                dec_ref_bits(_py, venv_site_packages_bits);
                return err;
            }
        };
        let stdlib_root_bits = match payload.stdlib_root.as_deref() {
            Some(root) => match alloc_str_bits(_py, root) {
                Ok(bits) => bits,
                Err(err) => {
                    dec_ref_bits(_py, resolved_paths_bits);
                    dec_ref_bits(_py, pythonpath_bits);
                    dec_ref_bits(_py, module_roots_bits);
                    dec_ref_bits(_py, venv_site_packages_bits);
                    dec_ref_bits(_py, pwd_bits);
                    return err;
                }
            },
            None => MoltObject::none().bits(),
        };
        let include_cwd_bits = MoltObject::from_bool(payload.include_cwd).bits();
        let keys_and_values: [(&[u8], u64); 7] = [
            (b"resolved_search_paths", resolved_paths_bits),
            (b"pythonpath_entries", pythonpath_bits),
            (b"module_roots_entries", module_roots_bits),
            (b"venv_site_packages_entries", venv_site_packages_bits),
            (b"pwd", pwd_bits),
            (b"include_cwd", include_cwd_bits),
            (b"stdlib_root", stdlib_root_bits),
        ];
        let mut pairs: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        let mut owned: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        for (key, value_bits) in keys_and_values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
            owned.push(value_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(dict_ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_importlib_search_paths(
    search_paths_bits: u64,
    module_file_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let search_paths =
            match string_sequence_arg_from_bits(_py, search_paths_bits, "search paths") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let resolved = importlib_search_paths(&search_paths, module_file);
        match alloc_string_list_bits(_py, &resolved) {
            Some(bits) => bits,
            None => raise_exception::<_>(_py, "MemoryError", "out of memory"),
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_importlib_namespace_paths(
    package_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let package = match string_arg_from_bits(_py, package_bits, "package") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let search_paths =
            match string_sequence_arg_from_bits(_py, search_paths_bits, "search paths") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let resolved = importlib_namespace_paths(&package, &search_paths, module_file);
        match alloc_string_list_bits(_py, &resolved) {
            Some(bits) => bits,
            None => raise_exception::<_>(_py, "MemoryError", "out of memory"),
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_importlib_resources_path_payload(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let payload = importlib_resources_path_payload(&path);
        let basename_bits = match alloc_str_bits(_py, &payload.basename) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let entries_bits = match alloc_string_list_bits(_py, &payload.entries) {
            Some(bits) => bits,
            None => {
                dec_ref_bits(_py, basename_bits);
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
        };
        let exists_bits = MoltObject::from_bool(payload.exists).bits();
        let is_file_bits = MoltObject::from_bool(payload.is_file).bits();
        let is_dir_bits = MoltObject::from_bool(payload.is_dir).bits();
        let has_init_py_bits = MoltObject::from_bool(payload.has_init_py).bits();
        let keys_and_values: [(&[u8], u64); 6] = [
            (b"basename", basename_bits),
            (b"exists", exists_bits),
            (b"is_file", is_file_bits),
            (b"is_dir", is_dir_bits),
            (b"entries", entries_bits),
            (b"has_init_py", has_init_py_bits),
        ];
        let mut pairs: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        let mut owned: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        for (key, value_bits) in keys_and_values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
            owned.push(value_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(dict_ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_importlib_resources_package_payload(
    package_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let package = match string_arg_from_bits(_py, package_bits, "package") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let search_paths =
            match string_sequence_arg_from_bits(_py, search_paths_bits, "search paths") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let payload = importlib_resources_package_payload(&package, &search_paths, module_file);
        let roots_bits = match alloc_string_list_bits(_py, &payload.roots) {
            Some(bits) => bits,
            None => return raise_exception::<_>(_py, "MemoryError", "out of memory"),
        };
        let init_file_bits = match payload.init_file.as_deref() {
            Some(path) => match alloc_str_bits(_py, path) {
                Ok(bits) => bits,
                Err(err) => {
                    dec_ref_bits(_py, roots_bits);
                    return err;
                }
            },
            None => MoltObject::none().bits(),
        };
        let is_namespace_bits = MoltObject::from_bool(payload.is_namespace).bits();
        let has_regular_package_bits = MoltObject::from_bool(payload.has_regular_package).bits();
        let keys_and_values: [(&[u8], u64); 4] = [
            (b"roots", roots_bits),
            (b"is_namespace", is_namespace_bits),
            (b"has_regular_package", has_regular_package_bits),
            (b"init_file", init_file_bits),
        ];
        let mut pairs: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        let mut owned: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        for (key, value_bits) in keys_and_values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
            owned.push(value_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(dict_ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_importlib_metadata_dist_paths(
    search_paths_bits: u64,
    module_file_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let search_paths =
            match string_sequence_arg_from_bits(_py, search_paths_bits, "search paths") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let resolved = importlib_metadata_dist_paths(&search_paths, module_file);
        match alloc_string_list_bits(_py, &resolved) {
            Some(bits) => bits,
            None => raise_exception::<_>(_py, "MemoryError", "out of memory"),
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_importlib_metadata_entry_points_payload(
    search_paths_bits: u64,
    module_file_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let search_paths =
            match string_sequence_arg_from_bits(_py, search_paths_bits, "search paths") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let payload = importlib_metadata_entry_points_payload(&search_paths, module_file);
        match alloc_string_triplets_list_bits(_py, &payload) {
            Some(bits) => bits,
            None => raise_exception::<_>(_py, "MemoryError", "out of memory"),
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_importlib_metadata_entry_points_select_payload(
    search_paths_bits: u64,
    module_file_bits: u64,
    group_bits: u64,
    name_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let search_paths =
            match string_sequence_arg_from_bits(_py, search_paths_bits, "search paths") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let group = match optional_string_arg_from_bits(_py, group_bits, "group") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let name = match optional_string_arg_from_bits(_py, name_bits, "name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let payload = importlib_metadata_entry_points_select_payload(
            &search_paths,
            module_file,
            group.as_deref(),
            name.as_deref(),
        );
        match alloc_string_triplets_list_bits(_py, &payload) {
            Some(bits) => bits,
            None => raise_exception::<_>(_py, "MemoryError", "out of memory"),
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_importlib_metadata_normalize_name(name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name = match string_arg_from_bits(_py, name_bits, "name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match alloc_str_bits(_py, &importlib_metadata_normalize_name(&name)) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_importlib_metadata_payload(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let payload = importlib_metadata_payload(&path);
        let path_bits = match alloc_str_bits(_py, &payload.path) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let name_bits = match alloc_str_bits(_py, &payload.name) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, path_bits);
                return err;
            }
        };
        let version_bits = match alloc_str_bits(_py, &payload.version) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, path_bits);
                dec_ref_bits(_py, name_bits);
                return err;
            }
        };
        let metadata_bits = match alloc_string_pairs_dict_bits(_py, &payload.metadata) {
            Some(bits) => bits,
            None => {
                dec_ref_bits(_py, path_bits);
                dec_ref_bits(_py, name_bits);
                dec_ref_bits(_py, version_bits);
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
        };
        let entry_points_bits = match alloc_string_triplets_list_bits(_py, &payload.entry_points) {
            Some(bits) => bits,
            None => {
                dec_ref_bits(_py, path_bits);
                dec_ref_bits(_py, name_bits);
                dec_ref_bits(_py, version_bits);
                dec_ref_bits(_py, metadata_bits);
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
        };
        let requires_dist_bits = match alloc_string_list_bits(_py, &payload.requires_dist) {
            Some(bits) => bits,
            None => {
                dec_ref_bits(_py, path_bits);
                dec_ref_bits(_py, name_bits);
                dec_ref_bits(_py, version_bits);
                dec_ref_bits(_py, metadata_bits);
                dec_ref_bits(_py, entry_points_bits);
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
        };
        let provides_extra_bits = match alloc_string_list_bits(_py, &payload.provides_extra) {
            Some(bits) => bits,
            None => {
                dec_ref_bits(_py, path_bits);
                dec_ref_bits(_py, name_bits);
                dec_ref_bits(_py, version_bits);
                dec_ref_bits(_py, metadata_bits);
                dec_ref_bits(_py, entry_points_bits);
                dec_ref_bits(_py, requires_dist_bits);
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
        };
        let requires_python_bits = match payload.requires_python.as_deref() {
            Some(value) => match alloc_str_bits(_py, value) {
                Ok(bits) => bits,
                Err(err) => {
                    dec_ref_bits(_py, path_bits);
                    dec_ref_bits(_py, name_bits);
                    dec_ref_bits(_py, version_bits);
                    dec_ref_bits(_py, metadata_bits);
                    dec_ref_bits(_py, entry_points_bits);
                    dec_ref_bits(_py, requires_dist_bits);
                    dec_ref_bits(_py, provides_extra_bits);
                    return err;
                }
            },
            None => MoltObject::none().bits(),
        };
        let keys_and_values: [(&[u8], u64); 8] = [
            (b"path", path_bits),
            (b"name", name_bits),
            (b"version", version_bits),
            (b"metadata", metadata_bits),
            (b"entry_points", entry_points_bits),
            (b"requires_dist", requires_dist_bits),
            (b"provides_extra", provides_extra_bits),
            (b"requires_python", requires_python_bits),
        ];
        let mut pairs: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        let mut owned: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        for (key, value_bits) in keys_and_values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
            owned.push(value_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(dict_ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_importlib_spec_from_file_location_payload(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let payload = importlib_spec_from_file_location_payload(&path);
        let path_bits = match alloc_str_bits(_py, &payload.path) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let package_root_bits = match payload.package_root.as_deref() {
            Some(root) => match alloc_str_bits(_py, root) {
                Ok(bits) => bits,
                Err(err) => {
                    dec_ref_bits(_py, path_bits);
                    return err;
                }
            },
            None => MoltObject::none().bits(),
        };
        let is_package_bits = MoltObject::from_bool(payload.is_package).bits();
        let keys_and_values: [(&[u8], u64); 3] = [
            (b"path", path_bits),
            (b"is_package", is_package_bits),
            (b"package_root", package_root_bits),
        ];
        let mut pairs: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        let mut owned: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        for (key, value_bits) in keys_and_values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
            owned.push(value_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(dict_ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_runpy_resolve_path(path_bits: u64, module_file_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let abs_path = bootstrap_resolve_abspath(&path, module_file);
        let is_file = std::fs::metadata(&abs_path)
            .map(|metadata| metadata.is_file())
            .unwrap_or(false);

        let abs_path_bits = match alloc_str_bits(_py, &abs_path) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let is_file_bits = MoltObject::from_bool(is_file).bits();
        let keys_and_values: [(&[u8], u64); 2] =
            [(b"abspath", abs_path_bits), (b"is_file", is_file_bits)];
        let mut pairs: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        let mut owned: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        for (key, value_bits) in keys_and_values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
            owned.push(value_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(dict_ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_os_name() -> u64 {
    crate::with_gil_entry!(_py, {
        init_atomic_bits(_py, &OS_NAME_CACHE, || {
            let ptr = alloc_string(_py, os_name_str().as_bytes());
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        })
    })
}

#[no_mangle]
pub extern "C" fn molt_sys_platform() -> u64 {
    crate::with_gil_entry!(_py, {
        init_atomic_bits(_py, &SYS_PLATFORM_CACHE, || {
            let ptr = alloc_string(_py, sys_platform_str().as_bytes());
            if ptr.is_null() {
                MoltObject::none().bits()
            } else {
                MoltObject::from_ptr(ptr).bits()
            }
        })
    })
}

#[no_mangle]
pub extern "C" fn molt_locale_setlocale(_category_bits: u64, locale_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if obj_from_bits(locale_bits).is_none() {
            let current = locale_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .clone();
            return match alloc_str_bits(_py, &current) {
                Ok(bits) => bits,
                Err(err_bits) => err_bits,
            };
        }
        let Some(mut locale) = string_obj_to_owned(obj_from_bits(locale_bits)) else {
            return raise_exception::<_>(_py, "TypeError", "locale must be str or None");
        };
        if locale.is_empty() || locale == "C" || locale == "POSIX" {
            locale = String::from("C");
        }
        *locale_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = locale.clone();
        match alloc_str_bits(_py, &locale) {
            Ok(bits) => bits,
            Err(err_bits) => err_bits,
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_locale_getpreferredencoding(_do_setlocale_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let current = locale_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        match alloc_str_bits(_py, locale_encoding_label(&current)) {
            Ok(bits) => bits,
            Err(err_bits) => err_bits,
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_locale_getlocale(_category_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let current = locale_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clone();
        if current == "C" || current == "POSIX" {
            let tuple_ptr =
                alloc_tuple(_py, &[MoltObject::none().bits(), MoltObject::none().bits()]);
            if tuple_ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            return MoltObject::from_ptr(tuple_ptr).bits();
        }
        let locale_bits = match alloc_str_bits(_py, &current) {
            Ok(bits) => bits,
            Err(err_bits) => return err_bits,
        };
        let encoding_bits = match alloc_str_bits(_py, locale_encoding_label(&current)) {
            Ok(bits) => bits,
            Err(err_bits) => {
                dec_ref_bits(_py, locale_bits);
                return err_bits;
            }
        };
        let tuple_ptr = alloc_tuple(_py, &[locale_bits, encoding_bits]);
        dec_ref_bits(_py, locale_bits);
        dec_ref_bits(_py, encoding_bits);
        if tuple_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(tuple_ptr).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_gettext_gettext(message_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        inc_ref_bits(_py, message_bits);
        message_bits
    })
}

#[no_mangle]
pub extern "C" fn molt_gettext_ngettext(singular_bits: u64, plural_bits: u64, n_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let one = MoltObject::from_int(1);
        let result_bits = if obj_eq(_py, obj_from_bits(n_bits), one) {
            singular_bits
        } else {
            plural_bits
        };
        inc_ref_bits(_py, result_bits);
        result_bits
    })
}

#[cfg(target_arch = "wasm32")]
fn collect_errno_constants() -> Vec<(&'static str, i64)> {
    vec![
        ("EACCES", libc::EACCES as i64),
        ("EAGAIN", libc::EAGAIN as i64),
        ("EALREADY", libc::EALREADY as i64),
        ("EBADF", libc::EBADF as i64),
        ("ECHILD", libc::ECHILD as i64),
        ("ECONNABORTED", libc::ECONNABORTED as i64),
        ("ECONNREFUSED", libc::ECONNREFUSED as i64),
        ("ECONNRESET", libc::ECONNRESET as i64),
        ("EEXIST", libc::EEXIST as i64),
        ("EHOSTUNREACH", libc::EHOSTUNREACH as i64),
        ("EINPROGRESS", libc::EINPROGRESS as i64),
        ("EINTR", libc::EINTR as i64),
        ("EINVAL", libc::EINVAL as i64),
        ("EISDIR", libc::EISDIR as i64),
        ("ENOENT", libc::ENOENT as i64),
        ("ENOTDIR", libc::ENOTDIR as i64),
        ("EPERM", libc::EPERM as i64),
        ("EPIPE", libc::EPIPE as i64),
        ("ESRCH", libc::ESRCH as i64),
        ("ETIMEDOUT", libc::ETIMEDOUT as i64),
        ("EWOULDBLOCK", libc::EWOULDBLOCK as i64),
    ]
}

#[cfg(not(target_arch = "wasm32"))]
include!(concat!(env!("OUT_DIR"), "/errno_constants.rs"));

fn socket_constants() -> Vec<(&'static str, i64)> {
    #[cfg(target_arch = "wasm32")]
    {
        return Vec::new();
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let mut out = vec![
            ("AF_INET", libc::AF_INET as i64),
            ("AF_INET6", libc::AF_INET6 as i64),
            ("SOCK_STREAM", libc::SOCK_STREAM as i64),
            ("SOCK_DGRAM", libc::SOCK_DGRAM as i64),
            ("SOCK_RAW", libc::SOCK_RAW as i64),
            ("SOL_SOCKET", libc::SOL_SOCKET as i64),
            ("SO_REUSEADDR", libc::SO_REUSEADDR as i64),
            ("SO_KEEPALIVE", libc::SO_KEEPALIVE as i64),
            ("SO_SNDBUF", libc::SO_SNDBUF as i64),
            ("SO_RCVBUF", libc::SO_RCVBUF as i64),
            ("SO_ERROR", libc::SO_ERROR as i64),
            ("SO_LINGER", libc::SO_LINGER as i64),
            ("SO_BROADCAST", libc::SO_BROADCAST as i64),
            ("IPPROTO_TCP", libc::IPPROTO_TCP as i64),
            ("IPPROTO_UDP", libc::IPPROTO_UDP as i64),
            ("IPPROTO_IPV6", libc::IPPROTO_IPV6 as i64),
            ("IPV6_V6ONLY", libc::IPV6_V6ONLY as i64),
            ("TCP_NODELAY", libc::TCP_NODELAY as i64),
            ("SHUT_RD", libc::SHUT_RD as i64),
            ("SHUT_WR", libc::SHUT_WR as i64),
            ("SHUT_RDWR", libc::SHUT_RDWR as i64),
            ("AI_PASSIVE", libc::AI_PASSIVE as i64),
            ("AI_CANONNAME", libc::AI_CANONNAME as i64),
            ("AI_NUMERICHOST", libc::AI_NUMERICHOST as i64),
            ("AI_NUMERICSERV", libc::AI_NUMERICSERV as i64),
            ("NI_NUMERICHOST", libc::NI_NUMERICHOST as i64),
            ("NI_NUMERICSERV", libc::NI_NUMERICSERV as i64),
            ("MSG_PEEK", libc::MSG_PEEK as i64),
        ];
        #[cfg(unix)]
        {
            out.push(("AF_UNIX", libc::AF_UNIX as i64));
        }
        #[cfg(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd",
            target_os = "dragonfly"
        ))]
        {
            out.push(("SCM_RIGHTS", libc::SCM_RIGHTS as i64));
        }
        #[cfg(unix)]
        {
            if SOCK_NONBLOCK_FLAG != 0 {
                out.push(("SOCK_NONBLOCK", SOCK_NONBLOCK_FLAG as i64));
            }
            if SOCK_CLOEXEC_FLAG != 0 {
                out.push(("SOCK_CLOEXEC", SOCK_CLOEXEC_FLAG as i64));
            }
        }
        #[cfg(unix)]
        {
            out.push(("MSG_DONTWAIT", libc::MSG_DONTWAIT as i64));
        }
        #[cfg(any(
            target_os = "linux",
            target_os = "android",
            target_os = "macos",
            target_os = "ios",
            target_os = "freebsd",
            target_os = "netbsd",
            target_os = "openbsd",
            target_os = "dragonfly"
        ))]
        {
            out.push(("SO_REUSEPORT", libc::SO_REUSEPORT as i64));
        }
        out.push(("EAI_AGAIN", libc::EAI_AGAIN as i64));
        out.push(("EAI_FAIL", libc::EAI_FAIL as i64));
        out.push(("EAI_FAMILY", libc::EAI_FAMILY as i64));
        out.push(("EAI_NONAME", libc::EAI_NONAME as i64));
        out.push(("EAI_SERVICE", libc::EAI_SERVICE as i64));
        out.push(("EAI_SOCKTYPE", libc::EAI_SOCKTYPE as i64));
        out
    }
}

#[no_mangle]
pub extern "C" fn molt_errno_constants() -> u64 {
    crate::with_gil_entry!(_py, {
        init_atomic_bits(_py, &ERRNO_CONSTANTS_CACHE, || {
            let constants = collect_errno_constants();
            let mut pairs = Vec::with_capacity(constants.len() * 2);
            let mut reverse_pairs = Vec::with_capacity(constants.len() * 2);
            let mut owned_bits = Vec::with_capacity(constants.len() * 2);
            for (name, value) in constants {
                let name_ptr = alloc_string(_py, name.as_bytes());
                if name_ptr.is_null() {
                    for bits in owned_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return MoltObject::none().bits();
                }
                let name_bits = MoltObject::from_ptr(name_ptr).bits();
                let value_bits = MoltObject::from_int(value).bits();
                pairs.push(name_bits);
                pairs.push(value_bits);
                reverse_pairs.push(value_bits);
                reverse_pairs.push(name_bits);
                owned_bits.push(name_bits);
                owned_bits.push(value_bits);
            }
            let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
            if dict_ptr.is_null() {
                for bits in owned_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let reverse_ptr = alloc_dict_with_pairs(_py, &reverse_pairs);
            if reverse_ptr.is_null() {
                dec_ref_bits(_py, MoltObject::from_ptr(dict_ptr).bits());
                for bits in owned_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
            let reverse_bits = MoltObject::from_ptr(reverse_ptr).bits();
            let tuple_ptr = alloc_tuple(_py, &[dict_bits, reverse_bits]);
            for bits in owned_bits {
                dec_ref_bits(_py, bits);
            }
            dec_ref_bits(_py, dict_bits);
            dec_ref_bits(_py, reverse_bits);
            if tuple_ptr.is_null() {
                return MoltObject::none().bits();
            }
            MoltObject::from_ptr(tuple_ptr).bits()
        })
    })
}

#[no_mangle]
pub extern "C" fn molt_socket_constants() -> u64 {
    crate::with_gil_entry!(_py, {
        init_atomic_bits(_py, &SOCKET_CONSTANTS_CACHE, || {
            let constants = socket_constants();
            let mut pairs = Vec::with_capacity(constants.len() * 2);
            let mut owned_bits = Vec::with_capacity(constants.len() * 2);
            for (name, value) in constants {
                let name_ptr = alloc_string(_py, name.as_bytes());
                if name_ptr.is_null() {
                    for bits in owned_bits {
                        dec_ref_bits(_py, bits);
                    }
                    return MoltObject::none().bits();
                }
                let name_bits = MoltObject::from_ptr(name_ptr).bits();
                let value_bits = MoltObject::from_int(value).bits();
                pairs.push(name_bits);
                pairs.push(value_bits);
                owned_bits.push(name_bits);
                owned_bits.push(value_bits);
            }
            let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
            if dict_ptr.is_null() {
                for bits in owned_bits {
                    dec_ref_bits(_py, bits);
                }
                return MoltObject::none().bits();
            }
            let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
            for bits in owned_bits {
                dec_ref_bits(_py, bits);
            }
            dict_bits
        })
    })
}

#[no_mangle]
pub extern "C" fn molt_env_get(key_bits: u64, default_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let key = match string_obj_to_owned(obj_from_bits(key_bits)) {
            Some(key) => key,
            None => return default_bits,
        };
        let value = {
            let guard = env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.get(&key).cloned()
        };
        match value {
            Some(val) => {
                if trace_env_get() {
                    eprintln!("molt_env_get key={key} hit=true");
                }
                let ptr = alloc_string(_py, val.as_bytes());
                if ptr.is_null() {
                    default_bits
                } else {
                    MoltObject::from_ptr(ptr).bits()
                }
            }
            None => {
                if trace_env_get() {
                    eprintln!("molt_env_get key={key} hit=false");
                }
                default_bits
            }
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_env_set(key_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let key = match string_obj_to_owned(obj_from_bits(key_bits)) {
            Some(key) => key,
            None => return MoltObject::none().bits(),
        };
        let value = match string_obj_to_owned(obj_from_bits(value_bits)) {
            Some(value) => value,
            None => return MoltObject::none().bits(),
        };
        {
            let mut guard = env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.insert(key, value);
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_env_unset(key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let key = match string_obj_to_owned(obj_from_bits(key_bits)) {
            Some(key) => key,
            None => return MoltObject::from_bool(false).bits(),
        };
        let removed = {
            let mut guard = env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.remove(&key).is_some()
        };
        MoltObject::from_bool(removed).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_env_len() -> u64 {
    crate::with_gil_entry!(_py, {
        let len = {
            let guard = env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.len()
        };
        MoltObject::from_int(len as i64).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_env_contains(key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let key = match string_obj_to_owned(obj_from_bits(key_bits)) {
            Some(key) => key,
            None => return MoltObject::from_bool(false).bits(),
        };
        let contains = {
            let guard = env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.contains_key(&key)
        };
        MoltObject::from_bool(contains).bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_env_snapshot() -> u64 {
    crate::with_gil_entry!(_py, {
        let env_pairs: Vec<(String, String)> = {
            let guard = env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard
                .iter()
                .map(|(key, val)| (key.clone(), val.clone()))
                .collect()
        };
        let mut pairs = Vec::with_capacity(env_pairs.len() * 2);
        let mut owned_bits = Vec::with_capacity(env_pairs.len() * 2);
        for (key, val) in env_pairs {
            let key_ptr = alloc_string(_py, key.as_bytes());
            let val_ptr = alloc_string(_py, val.as_bytes());
            if key_ptr.is_null() || val_ptr.is_null() {
                if !key_ptr.is_null() {
                    dec_ref_bits(_py, MoltObject::from_ptr(key_ptr).bits());
                }
                if !val_ptr.is_null() {
                    dec_ref_bits(_py, MoltObject::from_ptr(val_ptr).bits());
                }
                continue;
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            let val_bits = MoltObject::from_ptr(val_ptr).bits();
            pairs.push(key_bits);
            pairs.push(val_bits);
            owned_bits.push(key_bits);
            owned_bits.push(val_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        if dict_ptr.is_null() {
            for bits in owned_bits {
                dec_ref_bits(_py, bits);
            }
            return MoltObject::none().bits();
        }
        let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
        for bits in owned_bits {
            dec_ref_bits(_py, bits);
        }
        dict_bits
    })
}

#[no_mangle]
pub extern "C" fn molt_env_popitem() -> u64 {
    crate::with_gil_entry!(_py, {
        let (key, value) = {
            let mut guard = env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let Some((key, value)) = guard
                .iter()
                .next_back()
                .map(|(key, value)| (key.clone(), value.clone()))
            else {
                return raise_exception::<_>(_py, "KeyError", "popitem(): dictionary is empty");
            };
            guard.remove(&key);
            (key, value)
        };
        let key_ptr = alloc_string(_py, key.as_bytes());
        if key_ptr.is_null() {
            return MoltObject::none().bits();
        }
        let value_ptr = alloc_string(_py, value.as_bytes());
        if value_ptr.is_null() {
            dec_ref_bits(_py, MoltObject::from_ptr(key_ptr).bits());
            return MoltObject::none().bits();
        }
        let key_bits = MoltObject::from_ptr(key_ptr).bits();
        let value_bits = MoltObject::from_ptr(value_ptr).bits();
        let tuple_ptr = alloc_tuple(_py, &[key_bits, value_bits]);
        dec_ref_bits(_py, key_bits);
        dec_ref_bits(_py, value_bits);
        if tuple_ptr.is_null() {
            MoltObject::none().bits()
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[no_mangle]
pub extern "C" fn molt_env_clear() -> u64 {
    crate::with_gil_entry!(_py, {
        {
            let mut guard = env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.clear();
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_env_putenv(key_bits: u64, value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let key = match string_obj_to_owned(obj_from_bits(key_bits)) {
            Some(key) => key,
            None => return MoltObject::none().bits(),
        };
        let value = match string_obj_to_owned(obj_from_bits(value_bits)) {
            Some(value) => value,
            None => return MoltObject::none().bits(),
        };
        {
            let mut guard = process_env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.insert(key, value);
        }
        MoltObject::none().bits()
    })
}

#[no_mangle]
pub extern "C" fn molt_env_unsetenv(key_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let key = match string_obj_to_owned(obj_from_bits(key_bits)) {
            Some(key) => key,
            None => return MoltObject::none().bits(),
        };
        {
            let mut guard = process_env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            guard.remove(&key);
        }
        MoltObject::none().bits()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn with_env_state<R>(entries: &[(&str, &str)], f: impl FnOnce() -> R) -> R {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let original = {
            let mut env = env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let original = env.clone();
            env.clear();
            for (key, value) in entries {
                env.insert((*key).to_string(), (*value).to_string());
            }
            original
        };
        let out = f();
        {
            let mut env = env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            *env = original;
        }
        out
    }

    fn bootstrap_module_file() -> String {
        if sys_platform_str().starts_with("win") {
            "C:\\repo\\src\\molt\\stdlib\\sys.py".to_string()
        } else {
            "/repo/src/molt/stdlib/sys.py".to_string()
        }
    }

    fn expected_stdlib_root() -> String {
        if sys_platform_str().starts_with("win") {
            "C:\\repo\\src\\molt\\stdlib".to_string()
        } else {
            "/repo/src/molt/stdlib".to_string()
        }
    }

    #[test]
    fn sys_bootstrap_state_includes_pythonpath_module_roots_and_pwd() {
        let sep = if sys_platform_str().starts_with("win") {
            ';'
        } else {
            ':'
        };
        let py_path = format!("alpha{sep}beta");
        let module_roots = format!("gamma{sep}beta{sep}delta");
        with_env_state(
            &[
                ("PYTHONPATH", &py_path),
                ("MOLT_MODULE_ROOTS", &module_roots),
                ("MOLT_DEV_TRUSTED", "1"),
                ("PWD", "/tmp/molt_pwd"),
            ],
            || {
                let state = sys_bootstrap_state_from_module_file(Some(bootstrap_module_file()));
                assert_eq!(state.pythonpath_entries, vec!["alpha", "beta"]);
                assert_eq!(state.module_roots_entries, vec!["gamma", "beta", "delta"]);
                assert_eq!(state.stdlib_root, Some(expected_stdlib_root()));
                assert_eq!(state.pwd, "/tmp/molt_pwd");
                assert!(state.include_cwd);
                assert_eq!(
                    state.path,
                    vec![
                        "".to_string(),
                        "alpha".to_string(),
                        "beta".to_string(),
                        expected_stdlib_root(),
                        "gamma".to_string(),
                        "delta".to_string(),
                    ]
                );
            },
        );
    }

    #[test]
    fn sys_bootstrap_state_omits_cwd_when_dev_untrusted() {
        let sep = if sys_platform_str().starts_with("win") {
            ';'
        } else {
            ':'
        };
        let py_path = format!("alpha{sep}beta");
        with_env_state(
            &[
                ("PYTHONPATH", &py_path),
                ("MOLT_MODULE_ROOTS", ""),
                ("MOLT_DEV_TRUSTED", "0"),
                ("PWD", "/tmp/molt_pwd"),
            ],
            || {
                let state = sys_bootstrap_state_from_module_file(Some(bootstrap_module_file()));
                assert!(!state.include_cwd);
                assert!(!state.path.iter().any(|entry| entry.is_empty()));
                assert!(!state.path.iter().any(|entry| entry == "/tmp/molt_pwd"));
            },
        );
    }

    #[test]
    fn sys_bootstrap_state_falls_back_to_current_dir_when_pwd_missing() {
        with_env_state(
            &[
                ("PYTHONPATH", ""),
                ("MOLT_MODULE_ROOTS", ""),
                ("MOLT_DEV_TRUSTED", "1"),
                ("PWD", ""),
            ],
            || {
                let state = sys_bootstrap_state_from_module_file(Some(bootstrap_module_file()));
                assert!(state.include_cwd);
                assert_eq!(state.pwd, resolve_bootstrap_pwd(""));
                assert!(state.path.iter().any(|entry| entry.is_empty()));
            },
        );
    }

    #[test]
    fn sys_bootstrap_state_includes_virtual_env_site_packages_when_present() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_virtual_env_bootstrap_{}_{}",
            std::process::id(),
            stamp
        ));
        let venv_root = tmp.join("venv");
        let site_packages = if sys_platform_str().starts_with("win") {
            venv_root.join("Lib").join("site-packages")
        } else {
            venv_root
                .join("lib")
                .join("python3.12")
                .join("site-packages")
        };
        std::fs::create_dir_all(&site_packages).expect("create virtualenv site-packages");
        let venv_root_text = venv_root.to_string_lossy().into_owned();
        let site_packages_text = site_packages.to_string_lossy().into_owned();
        with_env_state(
            &[
                ("PYTHONPATH", ""),
                ("MOLT_MODULE_ROOTS", ""),
                ("VIRTUAL_ENV", &venv_root_text),
                ("MOLT_DEV_TRUSTED", "1"),
                ("PWD", "/tmp/molt_pwd"),
            ],
            || {
                let state = sys_bootstrap_state_from_module_file(Some(bootstrap_module_file()));
                assert_eq!(state.virtual_env_raw, venv_root_text);
                assert!(state
                    .venv_site_packages_entries
                    .iter()
                    .any(|entry| entry == &site_packages_text));
                assert!(state.path.iter().any(|entry| entry == &site_packages_text));
            },
        );
        std::fs::remove_dir_all(&tmp).expect("cleanup virtualenv temp dirs");
    }

    #[test]
    fn runpy_resolve_path_uses_bootstrap_pwd_for_relative_paths() {
        with_env_state(
            &[
                ("PYTHONPATH", ""),
                ("MOLT_MODULE_ROOTS", ""),
                ("MOLT_DEV_TRUSTED", "1"),
                ("PWD", "/tmp/bootstrap_pwd"),
            ],
            || {
                let resolved =
                    bootstrap_resolve_abspath("pkg/../mod.py", Some(bootstrap_module_file()));
                assert_eq!(resolved, "/tmp/bootstrap_pwd/mod.py");
            },
        );
    }

    #[test]
    fn importlib_source_loader_resolution_marks_packages() {
        let package = source_loader_resolution("demo.pkg", "/tmp/demo/pkg/__init__.py", false);
        assert!(package.is_package);
        assert_eq!(package.module_package, "demo.pkg");
        assert_eq!(package.package_root, Some("/tmp/demo/pkg".to_string()));

        let module = source_loader_resolution("demo.pkg.mod", "/tmp/demo/pkg/mod.py", false);
        assert!(!module.is_package);
        assert_eq!(module.module_package, "demo.pkg");
        assert_eq!(module.package_root, None);
    }

    #[test]
    fn importlib_source_exec_payload_reads_source_and_resolution() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_importlib_source_exec_payload_{}_{}",
            std::process::id(),
            stamp
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let module_path = tmp.join("demo.py");
        std::fs::write(&module_path, "value = 42\n").expect("write module source");

        let payload = importlib_source_exec_payload("demo", &module_path.to_string_lossy(), false)
            .expect("build source exec payload");
        assert!(!payload.is_package);
        assert_eq!(payload.module_package, "");
        assert_eq!(payload.package_root, None);
        let text = String::from_utf8(payload.source.clone()).expect("decode source text");
        assert!(text.contains("value = 42"));

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    }

    #[test]
    fn importlib_cache_from_source_matches_cpython_layout() {
        assert_eq!(
            importlib_cache_from_source("/tmp/pkg/mod.py"),
            "/tmp/pkg/__pycache__/mod.pyc"
        );
        assert_eq!(importlib_cache_from_source("/tmp/pkg/mod"), "/tmp/pkg/modc");
    }

    #[test]
    fn importlib_find_in_path_resolves_package_and_module() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_importlib_find_spec_{}_{}",
            std::process::id(),
            stamp
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let pkg_dir = tmp.join("pkgdemo");
        std::fs::create_dir_all(&pkg_dir).expect("create package dir");
        std::fs::write(pkg_dir.join("__init__.py"), "value = 1\n").expect("write __init__.py");
        std::fs::write(tmp.join("moddemo.py"), "value = 2\n").expect("write module file");

        let search_paths = vec![tmp.to_string_lossy().into_owned()];
        let pkg = importlib_find_in_path("pkgdemo", &search_paths).expect("package spec");
        assert!(pkg.is_package);
        assert!(pkg.origin.ends_with("__init__.py"));
        assert_eq!(
            pkg.submodule_search_locations,
            Some(vec![pkg_dir.to_string_lossy().into_owned()])
        );
        assert_eq!(pkg.cached, importlib_cache_from_source(&pkg.origin));

        let module = importlib_find_in_path("moddemo", &search_paths).expect("module spec");
        assert!(!module.is_package);
        assert!(module.origin.ends_with("moddemo.py"));
        assert_eq!(module.submodule_search_locations, None);
        assert_eq!(module.cached, importlib_cache_from_source(&module.origin));

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    }

    #[test]
    fn importlib_search_paths_includes_bootstrap_roots_and_stdlib_candidates() {
        let sep = if sys_platform_str().starts_with("win") {
            ';'
        } else {
            ':'
        };
        let module_roots = format!("vendor{sep}extra");
        with_env_state(
            &[
                ("PYTHONPATH", ""),
                ("MOLT_MODULE_ROOTS", &module_roots),
                ("MOLT_DEV_TRUSTED", "1"),
                ("PWD", "/tmp/bootstrap_pwd"),
            ],
            || {
                let resolved =
                    importlib_search_paths(&vec!["src".to_string()], Some(bootstrap_module_file()));
                assert!(resolved.iter().any(|entry| entry == "src"));
                assert!(resolved.iter().any(|entry| entry == "vendor"));
                assert!(resolved.iter().any(|entry| entry == "extra"));
                assert!(resolved.iter().any(|entry| {
                    entry.ends_with("/molt/stdlib") || entry.ends_with("\\molt\\stdlib")
                }));
                assert!(resolved
                    .iter()
                    .any(|entry| entry == &expected_stdlib_root()));
            },
        );
    }

    #[test]
    fn importlib_namespace_paths_finds_namespace_dirs() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_importlib_namespace_paths_{}_{}",
            std::process::id(),
            stamp
        ));
        let base_one = tmp.join("base_one");
        let base_two = tmp.join("base_two");
        let ns_one = base_one.join("nsdemo");
        let ns_two = base_two.join("nsdemo");
        std::fs::create_dir_all(&ns_one).expect("create namespace dir one");
        std::fs::create_dir_all(&ns_two).expect("create namespace dir two");
        let ns_one_text = ns_one.to_string_lossy().into_owned();
        let ns_two_text = ns_two.to_string_lossy().into_owned();
        let search_paths = vec![
            base_one.to_string_lossy().into_owned(),
            base_two.to_string_lossy().into_owned(),
        ];
        let resolved =
            importlib_namespace_paths("nsdemo", &search_paths, Some(bootstrap_module_file()));
        assert!(resolved.iter().any(|entry| entry == &ns_one_text));
        assert!(resolved.iter().any(|entry| entry == &ns_two_text));
        std::fs::remove_dir_all(&tmp).expect("cleanup temp dirs");
    }

    #[test]
    fn importlib_metadata_dist_paths_finds_dist_info_dirs() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_importlib_metadata_dist_paths_{}_{}",
            std::process::id(),
            stamp
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let dist_info = tmp.join("pkgdemo-1.0.dist-info");
        let egg_info = tmp.join("otherpkg-2.0.egg-info");
        std::fs::create_dir_all(&dist_info).expect("create dist-info dir");
        std::fs::create_dir_all(&egg_info).expect("create egg-info dir");
        std::fs::write(tmp.join("not_a_dist-info"), "x").expect("create plain file");
        let dist_info_text = dist_info.to_string_lossy().into_owned();
        let egg_info_text = egg_info.to_string_lossy().into_owned();
        let resolved = importlib_metadata_dist_paths(
            &vec![tmp.to_string_lossy().into_owned()],
            Some(bootstrap_module_file()),
        );
        assert!(resolved.iter().any(|entry| entry == &dist_info_text));
        assert!(resolved.iter().any(|entry| entry == &egg_info_text));
        assert!(
            !resolved
                .iter()
                .any(|entry| entry.ends_with("not_a_dist-info")),
            "non-dist-info file leaked into metadata path scan"
        );
        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    }

    #[test]
    fn importlib_resources_path_payload_reports_entries_and_init_marker() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_importlib_resources_payload_{}_{}",
            std::process::id(),
            stamp
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        std::fs::write(tmp.join("__init__.py"), "x = 1\n").expect("write __init__.py");
        std::fs::write(tmp.join("data.txt"), "payload\n").expect("write data.txt");

        let payload = importlib_resources_path_payload(&tmp.to_string_lossy());
        assert!(payload.exists);
        assert!(payload.is_dir);
        assert!(!payload.is_file);
        assert!(payload.has_init_py);
        assert!(payload.entries.iter().any(|entry| entry == "__init__.py"));
        assert!(payload.entries.iter().any(|entry| entry == "data.txt"));

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    }

    #[test]
    fn importlib_metadata_payload_parses_name_version_and_entry_points() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_importlib_metadata_payload_{}_{}",
            std::process::id(),
            stamp
        ));
        let dist = tmp.join("demo_pkg-1.2.3.dist-info");
        std::fs::create_dir_all(&dist).expect("create dist-info dir");
        std::fs::write(
            dist.join("METADATA"),
            "Name: demo-pkg\nVersion: 1.2.3\nSummary: demo\nRequires-Python: >=3.12\nRequires-Dist: dep-one>=1\nRequires-Dist: dep-two; extra == \"dev\"\nProvides-Extra: dev\n",
        )
        .expect("write metadata");
        std::fs::write(
            dist.join("entry_points.txt"),
            "[console_scripts]\ndemo = demo_pkg:main\n",
        )
        .expect("write entry points");

        let payload = importlib_metadata_payload(&dist.to_string_lossy());
        assert_eq!(payload.name, "demo-pkg");
        assert_eq!(payload.version, "1.2.3");
        assert!(payload
            .metadata
            .iter()
            .any(|(key, value)| key == "Name" && value == "demo-pkg"));
        assert!(payload.entry_points.iter().any(|(name, value, group)| {
            name == "demo" && value == "demo_pkg:main" && group == "console_scripts"
        }));
        assert_eq!(payload.requires_python.as_deref(), Some(">=3.12"));
        assert_eq!(payload.requires_dist.len(), 2);
        assert!(payload
            .requires_dist
            .iter()
            .any(|value| value == "dep-one>=1"));
        assert!(payload
            .requires_dist
            .iter()
            .any(|value| value == "dep-two; extra == \"dev\""));
        assert!(payload.provides_extra.iter().any(|value| value == "dev"));

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    }

    #[test]
    fn importlib_bootstrap_payload_reports_resolved_search_paths_and_env_fields() {
        let sep = if sys_platform_str().starts_with("win") {
            ';'
        } else {
            ':'
        };
        let module_roots = format!("vendor{sep}extra");
        with_env_state(
            &[
                ("PYTHONPATH", "alpha"),
                ("MOLT_MODULE_ROOTS", &module_roots),
                ("VIRTUAL_ENV", ""),
                ("MOLT_DEV_TRUSTED", "1"),
                ("PWD", "/tmp/bootstrap_pwd"),
            ],
            || {
                let payload = importlib_bootstrap_payload(
                    &vec!["src".to_string()],
                    Some(bootstrap_module_file()),
                );
                assert!(payload
                    .resolved_search_paths
                    .iter()
                    .any(|entry| entry == "src"));
                assert!(payload
                    .resolved_search_paths
                    .iter()
                    .any(|entry| entry == &expected_stdlib_root()));
                assert_eq!(payload.pythonpath_entries, vec!["alpha".to_string()]);
                assert!(payload
                    .module_roots_entries
                    .iter()
                    .any(|entry| entry == "vendor"));
                assert!(payload.venv_site_packages_entries.is_empty());
                assert!(payload.include_cwd);
                assert_eq!(payload.pwd, "/tmp/bootstrap_pwd");
            },
        );
    }

    #[test]
    fn importlib_metadata_entry_points_payload_aggregates_dist_entry_points() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_importlib_metadata_entry_points_payload_{}_{}",
            std::process::id(),
            stamp
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let dist_one = tmp.join("demo_one-1.0.dist-info");
        let dist_two = tmp.join("demo_two-2.0.dist-info");
        std::fs::create_dir_all(&dist_one).expect("create dist one");
        std::fs::create_dir_all(&dist_two).expect("create dist two");
        std::fs::write(
            dist_one.join("entry_points.txt"),
            "[console_scripts]\none = demo_one:main\n",
        )
        .expect("write entry_points one");
        std::fs::write(
            dist_two.join("entry_points.txt"),
            "[demo.group]\ntwo = demo_two:value\n",
        )
        .expect("write entry_points two");
        let payload = importlib_metadata_entry_points_payload(
            &vec![tmp.to_string_lossy().into_owned()],
            Some(bootstrap_module_file()),
        );
        assert!(payload.iter().any(|(name, value, group)| name == "one"
            && value == "demo_one:main"
            && group == "console_scripts"));
        assert!(payload.iter().any(|(name, value, group)| name == "two"
            && value == "demo_two:value"
            && group == "demo.group"));
        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    }

    #[test]
    fn importlib_metadata_entry_points_select_payload_filters_by_group_and_name() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_importlib_metadata_entry_points_select_payload_{}_{}",
            std::process::id(),
            stamp
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let dist = tmp.join("demo_select-1.0.dist-info");
        std::fs::create_dir_all(&dist).expect("create dist");
        std::fs::write(
            dist.join("entry_points.txt"),
            "[console_scripts]\nalpha = demo:alpha\nbeta = demo:beta\n[demo.group]\nalpha = demo:value\n",
        )
        .expect("write entry points");
        let search_paths = vec![tmp.to_string_lossy().into_owned()];
        let group_filtered = importlib_metadata_entry_points_select_payload(
            &search_paths,
            Some(bootstrap_module_file()),
            Some("console_scripts"),
            None,
        );
        assert_eq!(group_filtered.len(), 2);
        assert!(group_filtered
            .iter()
            .all(|(_, _, group)| group == "console_scripts"));
        let name_filtered = importlib_metadata_entry_points_select_payload(
            &search_paths,
            Some(bootstrap_module_file()),
            Some("console_scripts"),
            Some("beta"),
        );
        assert_eq!(name_filtered.len(), 1);
        assert_eq!(name_filtered[0].0, "beta");
        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    }

    #[test]
    fn importlib_metadata_normalize_name_collapses_separator_runs() {
        assert_eq!(
            importlib_metadata_normalize_name("Demo__payload---pkg.name"),
            "demo-payload-pkg-name"
        );
        assert_eq!(
            importlib_metadata_normalize_name("alpha...beta___gamma"),
            "alpha-beta-gamma"
        );
    }
}

#[cfg(target_arch = "wasm32")]
#[link(wasm_import_module = "wasi_snapshot_preview1")]
extern "C" {
    fn environ_sizes_get(environ_count: *mut u32, environ_buf_size: *mut u32) -> u16;
    fn environ_get(environ: *mut *mut u8, environ_buf: *mut u8) -> u16;
}
