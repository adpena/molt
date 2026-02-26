use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use digest::Digest;
use getrandom::fill as getrandom_fill;
use md5::Md5;
use serde_json::Value as JsonValue;
use sha1::Sha1;
use sha2::Sha256;

use crate::builtins::io::{
    path_basename_text, path_dirname_text, path_join_text, path_normpath_text,
};
use crate::builtins::modules::runpy_exec_restricted_source;
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

#[unsafe(no_mangle)]
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
static UUID_NODE_STATE: OnceLock<Mutex<Option<u64>>> = OnceLock::new();
static UUID_V1_STATE: OnceLock<Mutex<(Option<u16>, u64)>> = OnceLock::new();
static EXTENSION_METADATA_OK_CACHE: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
const UUID_EPOCH_100NS: u64 = 0x01B21DD213814000;

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

pub(crate) fn env_state_get(key: &str) -> Option<String> {
    let guard = env_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.get(key).cloned()
}

fn process_env_state() -> &'static Mutex<BTreeMap<String, String>> {
    PROCESS_ENV_STATE.get_or_init(|| Mutex::new(collect_env_state()))
}

fn locale_state() -> &'static Mutex<String> {
    LOCALE_STATE.get_or_init(|| Mutex::new(String::from("C")))
}

fn uuid_node_state() -> &'static Mutex<Option<u64>> {
    UUID_NODE_STATE.get_or_init(|| Mutex::new(None))
}

fn uuid_v1_state() -> &'static Mutex<(Option<u16>, u64)> {
    UUID_V1_STATE.get_or_init(|| Mutex::new((None, 0)))
}

fn extension_metadata_ok_cache() -> &'static Mutex<BTreeMap<String, String>> {
    EXTENSION_METADATA_OK_CACHE.get_or_init(|| Mutex::new(BTreeMap::new()))
}

fn uuid_random_bytes<const N: usize>() -> Result<[u8; N], String> {
    let mut out = [0u8; N];
    getrandom_fill(&mut out).map_err(|err| format!("os randomness unavailable: {err}"))?;
    Ok(out)
}

fn uuid_node() -> Result<u64, String> {
    let mut guard = uuid_node_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if let Some(node) = *guard {
        return Ok(node);
    }
    let mut bytes = uuid_random_bytes::<6>()?;
    // Match CPython behavior: set multicast bit on a random node id.
    bytes[0] |= 0x01;
    let mut node = 0u64;
    for byte in bytes {
        node = (node << 8) | u64::from(byte);
    }
    *guard = Some(node);
    Ok(node)
}

fn uuid_apply_version_and_variant(bytes: &mut [u8; 16], version: u8) {
    bytes[6] = (bytes[6] & 0x0F) | ((version & 0x0F) << 4);
    bytes[8] = (bytes[8] & 0x3F) | 0x80;
}

fn uuid_v4_bytes() -> Result<[u8; 16], String> {
    let mut bytes = uuid_random_bytes::<16>()?;
    uuid_apply_version_and_variant(&mut bytes, 4);
    Ok(bytes)
}

fn uuid_v3_bytes(namespace: &[u8], name: &[u8]) -> [u8; 16] {
    let mut hasher = Md5::new();
    hasher.update(namespace);
    hasher.update(name);
    let digest = hasher.finalize();
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest[..16]);
    uuid_apply_version_and_variant(&mut out, 3);
    out
}

fn uuid_v5_bytes(namespace: &[u8], name: &[u8]) -> [u8; 16] {
    let mut hasher = Sha1::new();
    hasher.update(namespace);
    hasher.update(name);
    let digest = hasher.finalize();
    let mut out = [0u8; 16];
    out.copy_from_slice(&digest[..16]);
    uuid_apply_version_and_variant(&mut out, 5);
    out
}

fn uuid_timestamp_100ns() -> u64 {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let ticks = now / 100;
    UUID_EPOCH_100NS.saturating_add(ticks as u64)
}

fn uuid_v1_bytes(
    node_override: Option<u64>,
    clock_seq_override: Option<u16>,
) -> Result<[u8; 16], String> {
    let node = match node_override {
        Some(value) => value,
        None => uuid_node()?,
    };
    let mut state = uuid_v1_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    if state.0.is_none() {
        let seed = uuid_random_bytes::<2>()?;
        state.0 = Some((u16::from(seed[0]) << 8 | u16::from(seed[1])) & 0x3FFF);
    }
    let mut timestamp = uuid_timestamp_100ns();
    let mut clock_seq = clock_seq_override.unwrap_or_else(|| state.0.unwrap_or(0));
    if timestamp <= state.1 {
        timestamp = state.1.saturating_add(1);
        if clock_seq_override.is_none() {
            clock_seq = (clock_seq + 1) & 0x3FFF;
        }
    }
    if clock_seq_override.is_none() {
        state.0 = Some(clock_seq);
    }
    state.1 = timestamp;
    drop(state);

    let time_low = (timestamp & 0xFFFF_FFFF) as u32;
    let time_mid = ((timestamp >> 32) & 0xFFFF) as u16;
    let mut time_hi_and_version = ((timestamp >> 48) & 0x0FFF) as u16;
    time_hi_and_version |= 1 << 12;
    let clock_seq_low = (clock_seq & 0xFF) as u8;
    let mut clock_seq_hi_and_reserved = ((clock_seq >> 8) & 0x3F) as u8;
    clock_seq_hi_and_reserved |= 0x80;

    let mut out = [0u8; 16];
    out[0..4].copy_from_slice(&time_low.to_be_bytes());
    out[4..6].copy_from_slice(&time_mid.to_be_bytes());
    out[6..8].copy_from_slice(&time_hi_and_version.to_be_bytes());
    out[8] = clock_seq_hi_and_reserved;
    out[9] = clock_seq_low;
    out[10] = ((node >> 40) & 0xFF) as u8;
    out[11] = ((node >> 32) & 0xFF) as u8;
    out[12] = ((node >> 24) & 0xFF) as u8;
    out[13] = ((node >> 16) & 0xFF) as u8;
    out[14] = ((node >> 8) & 0xFF) as u8;
    out[15] = (node & 0xFF) as u8;
    Ok(out)
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
        };
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
    let path_sep = if windows_paths { '\\' } else { '/' };
    let pwd = resolve_bootstrap_pwd(&pwd_raw);
    let mut pythonpath_entries: Vec<String> = Vec::new();
    for entry in split_nonempty_paths(&py_path_raw, sep) {
        let resolved = bootstrap_resolve_path_entry(&entry, &pwd, path_sep);
        append_unique_path(&mut pythonpath_entries, &resolved);
    }
    let mut paths: Vec<String> = pythonpath_entries.clone();

    let stdlib_root = module_file.and_then(|path| {
        if path.is_empty() {
            return None;
        }
        let dirname = path_dirname_text(&path, path_sep);
        if dirname.is_empty() {
            None
        } else {
            Some(bootstrap_resolve_path_entry(&dirname, &pwd, path_sep))
        }
    });
    if let Some(root) = &stdlib_root {
        append_unique_path(&mut paths, root);
    }

    let mut module_roots_entries: Vec<String> = Vec::new();
    for entry in split_nonempty_paths(&module_roots_raw, sep) {
        let resolved = bootstrap_resolve_path_entry(&entry, &pwd, path_sep);
        append_unique_path(&mut module_roots_entries, &resolved);
        append_unique_path(&mut paths, &resolved);
    }

    let venv_site_packages_entries =
        collect_virtual_env_site_packages(&virtual_env_raw, windows_paths);
    for entry in &venv_site_packages_entries {
        append_unique_path(&mut paths, entry);
    }

    let dev_trusted = dev_trusted_raw.trim().to_ascii_lowercase();
    let include_cwd = !matches!(dev_trusted.as_str(), "0" | "false" | "no");
    if include_cwd && !paths.iter().any(|entry| entry.is_empty()) {
        paths.insert(0, String::new());
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

struct ImportlibZipSourceExecPayload {
    source: Vec<u8>,
    is_package: bool,
    module_package: String,
    package_root: Option<String>,
    origin: String,
}

struct ImportlibPathResolution {
    origin: Option<String>,
    is_package: bool,
    submodule_search_locations: Option<Vec<String>>,
    cached: Option<String>,
    has_location: bool,
    loader_kind: String,
    zip_archive: Option<String>,
    zip_inner_path: Option<String>,
}

struct ImportlibFindSpecPayload {
    origin: Option<String>,
    is_package: bool,
    submodule_search_locations: Option<Vec<String>>,
    cached: Option<String>,
    is_builtin: bool,
    has_location: bool,
    loader_kind: String,
    zip_archive: Option<String>,
    zip_inner_path: Option<String>,
    meta_path_count: i64,
    path_hooks_count: i64,
}

struct ImportlibParentSearchPathsPayload {
    has_parent: bool,
    parent_name: Option<String>,
    search_paths: Vec<String>,
    needs_parent_spec: bool,
    package_context: bool,
}

struct ImportlibRuntimeStateViewBits {
    modules_bits: u64,
    meta_path_bits: u64,
    path_hooks_bits: u64,
    path_importer_cache_bits: u64,
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
    is_archive_member: bool,
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

struct ImportlibResourcesFilesPayload {
    package_name: String,
    roots: Vec<String>,
    is_namespace: bool,
    reader_bits: Option<u64>,
    files_traversable_bits: Option<u64>,
}

struct ImportlibMetadataRecordEntry {
    path: String,
    hash: Option<String>,
    size: Option<String>,
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

fn bootstrap_resolve_path_entry(path: &str, pwd: &str, sep: char) -> String {
    if path.is_empty() {
        return String::new();
    }
    if path_is_absolute_text(path, sep) || pwd.is_empty() {
        return path_normpath_text(path, sep);
    }
    path_normpath_text(&path_join_text(pwd.to_string(), path, sep), sep)
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

fn extension_loader_resolution(
    module_name: &str,
    path: &str,
    spec_is_package: bool,
) -> SourceLoaderResolution {
    let sep = bootstrap_path_sep();
    let basename = path_basename_text(path, sep).to_ascii_lowercase();
    let is_init = basename.starts_with("__init__.");
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

fn sourceless_loader_resolution(
    module_name: &str,
    path: &str,
    spec_is_package: bool,
) -> SourceLoaderResolution {
    let sep = bootstrap_path_sep();
    let basename = path_basename_text(path, sep).to_ascii_lowercase();
    let dirname = path_dirname_text(path, sep);
    let dirname_base = path_basename_text(&dirname, sep).to_ascii_lowercase();
    let is_cache_init = dirname_base == "__pycache__"
        && basename.starts_with("__init__.")
        && basename.ends_with(".pyc");
    let is_init = basename == "__init__.pyc" || is_cache_init;
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
        if is_cache_init {
            Some(path_dirname_text(&dirname, sep))
        } else {
            Some(path_dirname_text(path, sep))
        }
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

fn split_zip_archive_path(path: &str) -> Option<(String, String)> {
    const ARCHIVE_SUFFIXES: [&str; 3] = [".zip", ".whl", ".egg"];
    if path.is_empty() {
        return None;
    }
    let lower = path.to_ascii_lowercase();
    let mut best_idx: Option<usize> = None;
    let mut best_suffix_len: usize = 0;
    for suffix in ARCHIVE_SUFFIXES {
        let Some(idx) = lower.rfind(suffix) else {
            continue;
        };
        let archive_end = idx + suffix.len();
        if archive_end < path.len() {
            let tail = path.as_bytes()[archive_end];
            if tail != b'/' && tail != b'\\' {
                continue;
            }
        }
        if best_idx.is_none_or(|current| idx > current) {
            best_idx = Some(idx);
            best_suffix_len = suffix.len();
        }
    }
    let idx = best_idx?;
    let archive_end = idx + best_suffix_len;
    let archive = path[..archive_end].to_string();
    let remainder = path[archive_end..]
        .replace('\\', "/")
        .trim_matches('/')
        .to_string();
    Some((archive, remainder))
}

fn zip_entry_join(prefix: &str, rel: &str) -> String {
    if prefix.is_empty() {
        rel.to_string()
    } else {
        format!("{prefix}/{rel}")
    }
}

fn zip_archive_open(path: &str) -> Result<zip::ZipArchive<std::fs::File>, std::io::Error> {
    let file = std::fs::File::open(path)?;
    zip::ZipArchive::new(file)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string()))
}

fn zip_archive_entry_exists(path: &str, entry: &str) -> bool {
    let Ok(mut archive) = zip_archive_open(path) else {
        return false;
    };

    archive.by_name(entry).is_ok()
}

fn zip_archive_has_prefix(path: &str, prefix: &str) -> bool {
    let Ok(mut archive) = zip_archive_open(path) else {
        return false;
    };
    let mut normalized = prefix.replace('\\', "/");
    if !normalized.ends_with('/') {
        normalized.push('/');
    }
    for idx in 0..archive.len() {
        let Ok(file) = archive.by_index(idx) else {
            continue;
        };
        let name = file.name();
        if name.starts_with(&normalized) {
            return true;
        }
    }
    false
}

fn zip_archive_read_entry(path: &str, entry: &str) -> Result<Vec<u8>, std::io::Error> {
    let mut archive = zip_archive_open(path)?;
    let mut file = archive
        .by_name(entry)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::NotFound, err.to_string()))?;
    let mut bytes: Vec<u8> = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn zip_archive_resources_path_payload(
    archive_path: &str,
    inner_path: &str,
    basename: String,
) -> ImportlibResourcesPathPayload {
    let mut exists = false;
    let mut is_file = false;
    let mut is_dir = false;
    let mut entries: BTreeSet<String> = BTreeSet::new();
    let mut has_init_py = false;

    let normalized_inner = inner_path.replace('\\', "/").trim_matches('/').to_string();
    if normalized_inner.is_empty() {
        exists = true;
        is_dir = true;
    }

    let mut archive = match zip_archive_open(archive_path) {
        Ok(archive) => archive,
        Err(_) => {
            return ImportlibResourcesPathPayload {
                basename,
                exists: false,
                is_file: false,
                is_dir: false,
                entries: Vec::new(),
                has_init_py: false,
                is_archive_member: true,
            };
        }
    };

    let prefix = if normalized_inner.is_empty() {
        String::new()
    } else {
        format!("{normalized_inner}/")
    };

    for idx in 0..archive.len() {
        let Ok(file) = archive.by_index(idx) else {
            continue;
        };
        let mut name = file.name().replace('\\', "/");
        if name.is_empty() {
            continue;
        }
        let is_dir_entry = name.ends_with('/');
        name = name.trim_matches('/').to_string();
        if name.is_empty() {
            continue;
        }

        if !normalized_inner.is_empty() {
            if name == normalized_inner {
                exists = true;
                if is_dir_entry {
                    is_dir = true;
                } else {
                    is_file = true;
                }
                continue;
            }
            if !name.starts_with(&prefix) {
                continue;
            }
            exists = true;
            is_dir = true;
            let rel = &name[prefix.len()..];
            if rel.is_empty() {
                continue;
            }
            if let Some(child) = rel.split('/').next()
                && !child.is_empty()
            {
                entries.insert(child.to_string());
            }
            continue;
        }

        if let Some(child) = name.split('/').next()
            && !child.is_empty()
        {
            entries.insert(child.to_string());
        }
    }

    if !normalized_inner.is_empty() && !exists {
        if zip_archive_entry_exists(archive_path, &normalized_inner) {
            exists = true;
            is_file = true;
            is_dir = false;
        } else if zip_archive_has_prefix(archive_path, &normalized_inner) {
            exists = true;
            is_dir = true;
        }
    }

    let entries_vec: Vec<String> = entries.into_iter().collect();
    if is_dir {
        has_init_py = entries_vec.iter().any(|entry| entry == "__init__.py");
    }
    if is_file {
        has_init_py = false;
    }

    ImportlibResourcesPathPayload {
        basename,
        exists,
        is_file,
        is_dir,
        entries: entries_vec,
        has_init_py,
        is_archive_member: true,
    }
}

fn importlib_zip_source_exec_payload(
    module_name: &str,
    archive_path: &str,
    inner_path: &str,
    spec_is_package: bool,
) -> Result<ImportlibZipSourceExecPayload, std::io::Error> {
    let source_bytes = zip_archive_read_entry(archive_path, inner_path)?;
    let source =
        match crate::object::ops::decode_bytes_text("utf-8", "surrogateescape", &source_bytes) {
            Ok((text, _encoding)) => text,
            Err(_) => String::from_utf8_lossy(&source_bytes)
                .into_owned()
                .into_bytes(),
        };
    let origin = format!("{archive_path}/{inner_path}");
    let resolution = source_loader_resolution(module_name, &origin, spec_is_package);
    Ok(ImportlibZipSourceExecPayload {
        source,
        is_package: resolution.is_package,
        module_package: resolution.module_package,
        package_root: resolution.package_root,
        origin,
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

fn importlib_is_extension_filename(name: &str, module_name: &str) -> bool {
    if !name.starts_with(module_name) {
        return false;
    }
    let remainder = &name[module_name.len()..];
    if !remainder.starts_with('.') {
        return false;
    }
    if matches!(remainder, ".so" | ".pyd" | ".dll" | ".dylib") {
        return true;
    }
    (remainder.starts_with(".cpython-") && remainder.ends_with(".so"))
        || (remainder.starts_with(".abi") && remainder.ends_with(".so"))
        || (remainder.starts_with(".cp") && remainder.ends_with(".pyd"))
}

fn importlib_find_extension_module(base_dir: &str, module_name: &str) -> Option<String> {
    let dir = if base_dir.is_empty() { "." } else { base_dir };
    let read_dir = std::fs::read_dir(dir).ok()?;
    for entry in read_dir.flatten() {
        let file_name = entry.file_name().to_string_lossy().into_owned();
        if !importlib_is_extension_filename(&file_name, module_name) {
            continue;
        }
        let file_type = entry.file_type().ok()?;
        if !file_type.is_file() {
            continue;
        }
        return Some(entry.path().to_string_lossy().into_owned());
    }
    None
}

fn importlib_find_in_path(
    fullname: &str,
    search_paths: &[String],
    package_context: bool,
) -> Option<ImportlibPathResolution> {
    let sep = bootstrap_path_sep();
    let parts_all: Vec<&str> = fullname.split('.').collect();
    let parts: Vec<&str> = if package_context && parts_all.len() > 1 {
        vec![parts_all[parts_all.len() - 1]]
    } else {
        parts_all
    };
    if parts.is_empty() {
        return None;
    }
    let mut current_paths = search_paths.to_vec();
    for (idx, part) in parts.iter().enumerate() {
        let is_last = idx + 1 == parts.len();
        let mut found_pkg = false;
        let mut next_paths: Vec<String> = Vec::new();
        let mut namespace_paths: Vec<String> = Vec::new();
        for base in &current_paths {
            if let Some((zip_archive, zip_prefix)) = split_zip_archive_path(base) {
                let archive_is_file = std::fs::metadata(&zip_archive)
                    .map(|metadata| metadata.is_file())
                    .unwrap_or(false);
                if archive_is_file {
                    let pkg_rel = zip_entry_join(&zip_prefix, part);
                    let init_entry = format!("{pkg_rel}/__init__.py");
                    if zip_archive_entry_exists(&zip_archive, &init_entry) {
                        if is_last {
                            return Some(ImportlibPathResolution {
                                origin: Some(format!("{zip_archive}/{init_entry}")),
                                is_package: true,
                                submodule_search_locations: Some(vec![format!(
                                    "{zip_archive}/{pkg_rel}"
                                )]),
                                cached: None,
                                has_location: true,
                                loader_kind: "zip_source".to_string(),
                                zip_archive: Some(zip_archive),
                                zip_inner_path: Some(init_entry),
                            });
                        }
                        next_paths = vec![format!("{zip_archive}/{pkg_rel}")];
                        found_pkg = true;
                        break;
                    }
                    if zip_archive_has_prefix(&zip_archive, &pkg_rel) {
                        append_unique_path(
                            &mut namespace_paths,
                            &format!("{zip_archive}/{pkg_rel}"),
                        );
                    }
                    if is_last {
                        let mod_entry = zip_entry_join(&zip_prefix, &format!("{part}.py"));
                        if zip_archive_entry_exists(&zip_archive, &mod_entry) {
                            return Some(ImportlibPathResolution {
                                origin: Some(format!("{zip_archive}/{mod_entry}")),
                                is_package: false,
                                submodule_search_locations: None,
                                cached: None,
                                has_location: true,
                                loader_kind: "zip_source".to_string(),
                                zip_archive: Some(zip_archive),
                                zip_inner_path: Some(mod_entry),
                            });
                        }
                    }
                    continue;
                }
            }
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
                        origin: Some(init_file.clone()),
                        is_package: true,
                        submodule_search_locations: Some(vec![pkg_dir]),
                        cached: Some(importlib_cache_from_source(&init_file)),
                        has_location: true,
                        loader_kind: "source".to_string(),
                        zip_archive: None,
                        zip_inner_path: None,
                    });
                }
                next_paths = vec![pkg_dir];
                found_pkg = true;
                break;
            }
            let init_pyc = path_join_text(pkg_dir.clone(), "__init__.pyc", sep);
            if std::fs::metadata(&init_pyc)
                .map(|metadata| metadata.is_file())
                .unwrap_or(false)
            {
                if is_last {
                    return Some(ImportlibPathResolution {
                        origin: Some(init_pyc),
                        is_package: true,
                        submodule_search_locations: Some(vec![pkg_dir]),
                        cached: None,
                        has_location: true,
                        loader_kind: "bytecode".to_string(),
                        zip_archive: None,
                        zip_inner_path: None,
                    });
                }
                next_paths = vec![pkg_dir];
                found_pkg = true;
                break;
            }
            if std::fs::metadata(&pkg_dir)
                .map(|metadata| metadata.is_dir())
                .unwrap_or(false)
            {
                append_unique_path(&mut namespace_paths, &pkg_dir);
            }
            if is_last {
                let mod_file = path_join_text(root.clone(), &format!("{part}.py"), sep);
                if std::fs::metadata(&mod_file)
                    .map(|metadata| metadata.is_file())
                    .unwrap_or(false)
                {
                    return Some(ImportlibPathResolution {
                        origin: Some(mod_file.clone()),
                        is_package: false,
                        submodule_search_locations: None,
                        cached: Some(importlib_cache_from_source(&mod_file)),
                        has_location: true,
                        loader_kind: "source".to_string(),
                        zip_archive: None,
                        zip_inner_path: None,
                    });
                }
                if let Some(ext_file) = importlib_find_extension_module(base, part) {
                    return Some(ImportlibPathResolution {
                        origin: Some(ext_file),
                        is_package: false,
                        submodule_search_locations: None,
                        cached: None,
                        has_location: true,
                        loader_kind: "extension".to_string(),
                        zip_archive: None,
                        zip_inner_path: None,
                    });
                }
                let bytecode_file = path_join_text(root, &format!("{part}.pyc"), sep);
                if std::fs::metadata(&bytecode_file)
                    .map(|metadata| metadata.is_file())
                    .unwrap_or(false)
                {
                    return Some(ImportlibPathResolution {
                        origin: Some(bytecode_file),
                        is_package: false,
                        submodule_search_locations: None,
                        cached: None,
                        has_location: true,
                        loader_kind: "bytecode".to_string(),
                        zip_archive: None,
                        zip_inner_path: None,
                    });
                }
            }
        }
        if found_pkg {
            current_paths = next_paths;
            continue;
        }
        if !namespace_paths.is_empty() {
            if is_last {
                return Some(ImportlibPathResolution {
                    origin: None,
                    is_package: true,
                    submodule_search_locations: Some(namespace_paths),
                    cached: None,
                    has_location: false,
                    loader_kind: "namespace".to_string(),
                    zip_archive: None,
                    zip_inner_path: None,
                });
            }
            current_paths = namespace_paths;
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
        if let Some((archive_path, zip_prefix)) = split_zip_archive_path(&base) {
            let archive_exists = std::fs::metadata(&archive_path)
                .map(|metadata| metadata.is_file())
                .unwrap_or(false);
            if archive_exists {
                let mut rel = zip_prefix;
                for part in &parts {
                    rel = zip_entry_join(&rel, part);
                }
                if zip_archive_has_prefix(&archive_path, &rel) {
                    append_unique_path(&mut matches, &format!("{archive_path}/{rel}"));
                }
                continue;
            }
        }
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

fn importlib_metadata_distributions_payload(
    search_paths: &[String],
    module_file: Option<String>,
) -> Vec<ImportlibMetadataPayload> {
    let dist_paths = importlib_metadata_dist_paths(search_paths, module_file);
    let mut out: Vec<ImportlibMetadataPayload> = Vec::with_capacity(dist_paths.len());
    for path in dist_paths {
        out.push(importlib_metadata_payload(&path));
    }
    out
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
            if let Some(expected_group) = group
                && entry_group != expected_group
            {
                continue;
            }
            if let Some(expected_name) = name
                && entry_name != expected_name
            {
                continue;
            }
            out.push((entry_name, entry_value, entry_group));
        }
    }
    out
}

fn importlib_metadata_entry_points_filter_payload(
    search_paths: &[String],
    module_file: Option<String>,
    group: Option<&str>,
    name: Option<&str>,
    value: Option<&str>,
) -> Vec<(String, String, String)> {
    let mut out: Vec<(String, String, String)> = Vec::new();
    let dist_paths = importlib_metadata_dist_paths(search_paths, module_file);
    for path in dist_paths {
        let payload = importlib_metadata_payload(&path);
        for (entry_name, entry_value, entry_group) in payload.entry_points {
            if let Some(expected_group) = group
                && entry_group != expected_group
            {
                continue;
            }
            if let Some(expected_name) = name
                && entry_name != expected_name
            {
                continue;
            }
            if let Some(expected_value) = value
                && entry_value != expected_value
            {
                continue;
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
    package_context: bool,
) -> Option<ImportlibFindSpecPayload> {
    let resolved = importlib_search_paths(search_paths, module_file);
    let resolution = importlib_find_in_path(fullname, &resolved, package_context)?;
    Some(ImportlibFindSpecPayload {
        origin: resolution.origin,
        is_package: resolution.is_package,
        submodule_search_locations: resolution.submodule_search_locations,
        cached: resolution.cached,
        is_builtin: false,
        has_location: resolution.has_location,
        loader_kind: resolution.loader_kind,
        zip_archive: resolution.zip_archive,
        zip_inner_path: resolution.zip_inner_path,
        meta_path_count,
        path_hooks_count,
    })
}

fn pending_exception_kind_name(_py: &PyToken<'_>) -> Option<String> {
    if !exception_pending(_py) {
        return None;
    }
    let exc_bits = molt_exception_last();
    let kind = maybe_ptr_from_bits(exc_bits)
        .and_then(|ptr| string_obj_to_owned(obj_from_bits(unsafe { exception_kind_bits(ptr) })));
    if !obj_from_bits(exc_bits).is_none() {
        dec_ref_bits(_py, exc_bits);
    }
    kind
}

fn clear_pending_if_kind(_py: &PyToken<'_>, kinds: &[&str]) -> bool {
    let Some(kind) = pending_exception_kind_name(_py) else {
        return false;
    };
    if kinds.iter().any(|value| *value == kind) {
        clear_exception(_py);
        return true;
    }
    false
}

fn call_callable_positional(
    _py: &PyToken<'_>,
    callable_bits: u64,
    args: &[u64],
) -> Result<u64, u64> {
    // Use direct call entrypoints for common arities to preserve callable
    // binding semantics and returned objects exactly.
    match args.len() {
        0 => return Ok(unsafe { call_callable0(_py, callable_bits) }),
        1 => return Ok(unsafe { call_callable1(_py, callable_bits, args[0]) }),
        2 => return Ok(unsafe { call_callable2(_py, callable_bits, args[0], args[1]) }),
        3 => return Ok(unsafe { call_callable3(_py, callable_bits, args[0], args[1], args[2]) }),
        4 => {}
        _ => {}
    }
    let builder_bits = molt_callargs_new(args.len() as u64, 0);
    if builder_bits == 0 {
        return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
    }
    for &arg in args {
        let _ = unsafe { molt_callargs_push_pos(builder_bits, arg) };
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
    }
    Ok(molt_call_bind(callable_bits, builder_bits))
}

fn call_meta_finder_find_spec(
    _py: &PyToken<'_>,
    finder_bits: u64,
    finder_method_bits: u64,
    fullname_bits: u64,
    search_paths_bits: u64,
) -> Result<u64, u64> {
    let none_bits = MoltObject::none().bits();
    // Attempt CPython-style signatures in order from most common bound-call
    // forms to unbound descriptor forms.
    let attempts: [&[u64]; 10] = [
        &[fullname_bits, none_bits, none_bits],
        &[fullname_bits, none_bits],
        &[fullname_bits, search_paths_bits, none_bits],
        &[fullname_bits, search_paths_bits],
        &[fullname_bits],
        &[finder_bits, fullname_bits, none_bits, none_bits],
        &[finder_bits, fullname_bits, none_bits],
        &[finder_bits, fullname_bits, search_paths_bits, none_bits],
        &[finder_bits, fullname_bits, search_paths_bits],
        &[finder_bits, fullname_bits], // legacy unbound fallback
    ];
    for args in attempts {
        let result = call_callable_positional(_py, finder_method_bits, args)?;
        if !exception_pending(_py) {
            return Ok(result);
        }
        if !clear_pending_if_kind(_py, &["TypeError"]) {
            return Err(MoltObject::none().bits());
        }
    }
    Err(MoltObject::none().bits())
}

#[allow(dead_code)]
fn call_path_entry_finder_find_spec(
    _py: &PyToken<'_>,
    finder_bits: u64,
    finder_method_bits: u64,
    fullname_bits: u64,
) -> Result<u64, u64> {
    let none_bits = MoltObject::none().bits();
    let attempts: [&[u64]; 4] = [
        &[fullname_bits, none_bits],
        &[fullname_bits],
        &[finder_bits, fullname_bits, none_bits],
        &[finder_bits, fullname_bits],
    ];
    for args in attempts {
        let result = call_callable_positional(_py, finder_method_bits, args)?;
        if !exception_pending(_py) {
            return Ok(result);
        }
        if !clear_pending_if_kind(_py, &["TypeError"]) {
            return Err(MoltObject::none().bits());
        }
    }
    Err(MoltObject::none().bits())
}

fn find_spec_method_bits(
    _py: &PyToken<'_>,
    finder_bits: u64,
    find_spec_name: u64,
) -> Result<Option<u64>, u64> {
    let missing = missing_bits(_py);
    let method_bits = molt_getattr_builtin(finder_bits, find_spec_name, missing);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if is_missing_bits(_py, method_bits) {
        return Ok(None);
    }
    // Preserve the method object exactly as retrieved from getattr and let the
    // call-site attempt bound/unbound signatures explicitly.
    Ok(Some(method_bits))
}

fn importlib_find_spec_via_meta_path(
    _py: &PyToken<'_>,
    fullname: &str,
    search_paths: &[String],
    meta_path_bits: u64,
) -> Result<Option<u64>, u64> {
    if obj_from_bits(meta_path_bits).is_none() {
        return Ok(None);
    }
    let fullname_bits = alloc_str_bits(_py, fullname)?;
    let search_paths_bits = match alloc_string_list_bits(_py, search_paths) {
        Some(bits) => bits,
        None => {
            dec_ref_bits(_py, fullname_bits);
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
    };
    let result = (|| {
        static FIND_SPEC_NAME: AtomicU64 = AtomicU64::new(0);
        let find_spec_name = intern_static_name(_py, &FIND_SPEC_NAME, b"find_spec");
        let iter_bits = molt_iter(meta_path_bits);
        if exception_pending(_py) {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "meta_path must be iterable",
            ));
        }
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
            let finder_bits = pair[0];
            if maybe_ptr_from_bits(finder_bits).is_none() {
                continue;
            }
            let Some(find_spec_bits) = find_spec_method_bits(_py, finder_bits, find_spec_name)?
            else {
                continue;
            };
            let spec_bits = match call_meta_finder_find_spec(
                _py,
                finder_bits,
                find_spec_bits,
                fullname_bits,
                search_paths_bits,
            ) {
                Ok(bits) => bits,
                Err(err) => {
                    dec_ref_bits(_py, find_spec_bits);
                    return Err(err);
                }
            };
            dec_ref_bits(_py, find_spec_bits);
            if !obj_from_bits(spec_bits).is_none() {
                return Ok(Some(spec_bits));
            }
        }
        Ok(None)
    })();
    dec_ref_bits(_py, fullname_bits);
    dec_ref_bits(_py, search_paths_bits);
    result
}

#[allow(dead_code)]
fn importlib_find_spec_via_path_hooks(
    _py: &PyToken<'_>,
    fullname: &str,
    search_paths: &[String],
    path_hooks_bits: u64,
    path_importer_cache_bits: u64,
) -> Result<Option<u64>, u64> {
    if obj_from_bits(path_hooks_bits).is_none() {
        return Ok(None);
    }
    let path_importer_cache_ptr = if obj_from_bits(path_importer_cache_bits).is_none() {
        None
    } else {
        let Some(cache_ptr) = maybe_ptr_from_bits(path_importer_cache_bits) else {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "path_importer_cache must be dict or None",
            ));
        };
        unsafe {
            if object_type_id(cache_ptr) != TYPE_ID_DICT {
                return Err(raise_exception::<_>(
                    _py,
                    "TypeError",
                    "path_importer_cache must be dict or None",
                ));
            }
        }
        Some(cache_ptr)
    };
    let fullname_bits = alloc_str_bits(_py, fullname)?;
    let result = (|| {
        static FIND_SPEC_NAME: AtomicU64 = AtomicU64::new(0);
        let find_spec_name = intern_static_name(_py, &FIND_SPEC_NAME, b"find_spec");
        let hooks_iter = molt_iter(path_hooks_bits);
        if exception_pending(_py) {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "path_hooks must be iterable",
            ));
        }
        let mut hooks: Vec<u64> = Vec::new();
        loop {
            let pair_bits = molt_iter_next(hooks_iter);
            let Some(pair_ptr) = maybe_ptr_from_bits(pair_bits) else {
                for hook_bits in hooks {
                    dec_ref_bits(_py, hook_bits);
                }
                return Err(MoltObject::none().bits());
            };
            unsafe {
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    for hook_bits in hooks {
                        dec_ref_bits(_py, hook_bits);
                    }
                    return Err(MoltObject::none().bits());
                }
            }
            let pair = unsafe { seq_vec_ref(pair_ptr) };
            if pair.len() < 2 {
                for hook_bits in hooks {
                    dec_ref_bits(_py, hook_bits);
                }
                return Err(MoltObject::none().bits());
            }
            if is_truthy(_py, obj_from_bits(pair[1])) {
                break;
            }
            if obj_from_bits(pair[0]).is_none() {
                continue;
            }
            inc_ref_bits(_py, pair[0]);
            hooks.push(pair[0]);
        }
        for entry in search_paths {
            let entry_bits = alloc_str_bits(_py, entry)?;
            let mut finder_bits = MoltObject::none().bits();
            let mut finder_owned = false;
            if let Some(cache_ptr) = path_importer_cache_ptr
                && let Some(cached_bits) = unsafe { dict_get_in_place(_py, cache_ptr, entry_bits) }
            {
                if obj_from_bits(cached_bits).is_none() {
                    dec_ref_bits(_py, entry_bits);
                    continue;
                }
                finder_bits = cached_bits;
            }

            if obj_from_bits(finder_bits).is_none() {
                for hook_bits in &hooks {
                    finder_bits = unsafe { call_callable1(_py, *hook_bits, entry_bits) };
                    if exception_pending(_py) {
                        if clear_pending_if_kind(_py, &["ImportError", "ModuleNotFoundError"]) {
                            continue;
                        }
                        dec_ref_bits(_py, entry_bits);
                        for hook_bits in hooks {
                            dec_ref_bits(_py, hook_bits);
                        }
                        return Err(MoltObject::none().bits());
                    }
                    if obj_from_bits(finder_bits).is_none() {
                        continue;
                    }
                    finder_owned = true;
                    if let Some(cache_ptr) = path_importer_cache_ptr {
                        unsafe { dict_set_in_place(_py, cache_ptr, entry_bits, finder_bits) };
                        if exception_pending(_py) {
                            if finder_owned && !obj_from_bits(finder_bits).is_none() {
                                dec_ref_bits(_py, finder_bits);
                            }
                            dec_ref_bits(_py, entry_bits);
                            for hook_bits in hooks {
                                dec_ref_bits(_py, hook_bits);
                            }
                            return Err(MoltObject::none().bits());
                        }
                    }
                    break;
                }
                if obj_from_bits(finder_bits).is_none() {
                    if let Some(cache_ptr) = path_importer_cache_ptr {
                        unsafe {
                            dict_set_in_place(_py, cache_ptr, entry_bits, MoltObject::none().bits())
                        };
                        if exception_pending(_py) {
                            dec_ref_bits(_py, entry_bits);
                            for hook_bits in hooks {
                                dec_ref_bits(_py, hook_bits);
                            }
                            return Err(MoltObject::none().bits());
                        }
                    }
                    dec_ref_bits(_py, entry_bits);
                    continue;
                }
            }

            if maybe_ptr_from_bits(finder_bits).is_none() {
                if finder_owned && !obj_from_bits(finder_bits).is_none() {
                    dec_ref_bits(_py, finder_bits);
                }
                dec_ref_bits(_py, entry_bits);
                continue;
            }
            let find_spec_bits = match find_spec_method_bits(_py, finder_bits, find_spec_name) {
                Ok(Some(bits)) => bits,
                Ok(None) => {
                    if finder_owned && !obj_from_bits(finder_bits).is_none() {
                        dec_ref_bits(_py, finder_bits);
                    }
                    dec_ref_bits(_py, entry_bits);
                    continue;
                }
                Err(err) => {
                    if finder_owned && !obj_from_bits(finder_bits).is_none() {
                        dec_ref_bits(_py, finder_bits);
                    }
                    dec_ref_bits(_py, entry_bits);
                    for hook_bits in hooks {
                        dec_ref_bits(_py, hook_bits);
                    }
                    return Err(err);
                }
            };
            let spec_bits = match call_path_entry_finder_find_spec(
                _py,
                finder_bits,
                find_spec_bits,
                fullname_bits,
            ) {
                Ok(bits) => bits,
                Err(err) => {
                    dec_ref_bits(_py, find_spec_bits);
                    if finder_owned && !obj_from_bits(finder_bits).is_none() {
                        dec_ref_bits(_py, finder_bits);
                    }
                    dec_ref_bits(_py, entry_bits);
                    for hook_bits in &hooks {
                        dec_ref_bits(_py, *hook_bits);
                    }
                    return Err(err);
                }
            };
            dec_ref_bits(_py, find_spec_bits);
            if finder_owned && !obj_from_bits(finder_bits).is_none() {
                dec_ref_bits(_py, finder_bits);
            }
            if !obj_from_bits(spec_bits).is_none() {
                dec_ref_bits(_py, entry_bits);
                for hook_bits in hooks {
                    dec_ref_bits(_py, hook_bits);
                }
                return Ok(Some(spec_bits));
            }
            dec_ref_bits(_py, entry_bits);
        }
        for hook_bits in hooks {
            dec_ref_bits(_py, hook_bits);
        }
        Ok(None)
    })();
    dec_ref_bits(_py, fullname_bits);
    result
}

fn importlib_find_spec_direct_payload_bits(
    _py: &PyToken<'_>,
    spec_bits: u64,
    meta_path_count: i64,
    path_hooks_count: i64,
) -> Result<u64, u64> {
    let loader_kind_bits = alloc_str_bits(_py, "direct")?;
    let meta_path_count_bits = int_bits_from_i64(_py, meta_path_count);
    let path_hooks_count_bits = int_bits_from_i64(_py, path_hooks_count);
    let keys_and_values: [(&[u8], u64); 4] = [
        (b"spec", spec_bits),
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
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
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
        Err(raise_exception::<_>(_py, "MemoryError", "out of memory"))
    } else {
        Ok(MoltObject::from_ptr(dict_ptr).bits())
    }
}

fn importlib_runtime_state_attr_bits(
    _py: &PyToken<'_>,
    sys_bits: u64,
    slot: &AtomicU64,
    name: &'static [u8],
) -> Result<u64, u64> {
    let attr_name = intern_static_name(_py, slot, name);
    let missing = missing_bits(_py);
    let attr_bits = molt_getattr_builtin(sys_bits, attr_name, missing);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if is_missing_bits(_py, attr_bits) {
        return Ok(MoltObject::none().bits());
    }
    Ok(attr_bits)
}

fn importlib_runtime_state_payload_bits(_py: &PyToken<'_>) -> Result<u64, u64> {
    static MODULES_NAME: AtomicU64 = AtomicU64::new(0);
    static META_PATH_NAME: AtomicU64 = AtomicU64::new(0);
    static PATH_HOOKS_NAME: AtomicU64 = AtomicU64::new(0);
    static PATH_IMPORTER_CACHE_NAME: AtomicU64 = AtomicU64::new(0);

    let mut modules_bits = MoltObject::none().bits();
    let mut meta_path_bits = MoltObject::none().bits();
    let mut path_hooks_bits = MoltObject::none().bits();
    let mut path_importer_cache_bits = MoltObject::none().bits();

    let sys_bits = {
        let cache = crate::builtins::exceptions::internals::module_cache(_py);
        let guard = cache.lock().unwrap();
        guard.get("sys").copied()
    };

    if let Some(sys_bits) = sys_bits
        && !obj_from_bits(sys_bits).is_none()
    {
        modules_bits = importlib_runtime_state_attr_bits(_py, sys_bits, &MODULES_NAME, b"modules")?;
        meta_path_bits =
            importlib_runtime_state_attr_bits(_py, sys_bits, &META_PATH_NAME, b"meta_path")?;
        path_hooks_bits =
            importlib_runtime_state_attr_bits(_py, sys_bits, &PATH_HOOKS_NAME, b"path_hooks")?;
        path_importer_cache_bits = importlib_runtime_state_attr_bits(
            _py,
            sys_bits,
            &PATH_IMPORTER_CACHE_NAME,
            b"path_importer_cache",
        )?;
    }

    let dict_ptr = alloc_dict_with_pairs(_py, &[]);
    if dict_ptr.is_null() {
        for bits in [
            modules_bits,
            meta_path_bits,
            path_hooks_bits,
            path_importer_cache_bits,
        ] {
            if !obj_from_bits(bits).is_none() {
                dec_ref_bits(_py, bits);
            }
        }
        return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
    }
    let dict_bits = MoltObject::from_ptr(dict_ptr).bits();
    let entries = [
        (
            intern_static_name(_py, &MODULES_NAME, b"modules"),
            modules_bits,
        ),
        (
            intern_static_name(_py, &META_PATH_NAME, b"meta_path"),
            meta_path_bits,
        ),
        (
            intern_static_name(_py, &PATH_HOOKS_NAME, b"path_hooks"),
            path_hooks_bits,
        ),
        (
            intern_static_name(_py, &PATH_IMPORTER_CACHE_NAME, b"path_importer_cache"),
            path_importer_cache_bits,
        ),
    ];
    for (key_bits, value_bits) in entries {
        unsafe {
            dict_set_in_place(_py, dict_ptr, key_bits, value_bits);
        }
        if exception_pending(_py) {
            for (_, bits) in entries {
                if !obj_from_bits(bits).is_none() {
                    dec_ref_bits(_py, bits);
                }
            }
            dec_ref_bits(_py, dict_bits);
            return Err(MoltObject::none().bits());
        }
    }
    for (_, bits) in entries {
        if !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
    }
    Ok(dict_bits)
}

fn importlib_resources_path_payload(path: &str) -> ImportlibResourcesPathPayload {
    let sep = bootstrap_path_sep();
    let basename = path_basename_text(path, sep);
    if let Some((archive_path, inner_path)) = split_zip_archive_path(path) {
        let archive_exists = std::fs::metadata(&archive_path)
            .map(|metadata| metadata.is_file())
            .unwrap_or(false);
        if archive_exists {
            return zip_archive_resources_path_payload(&archive_path, &inner_path, basename);
        }
    }
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
    if is_dir && let Ok(read_dir) = std::fs::read_dir(path) {
        for entry in read_dir.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if name == "__init__.py" {
                has_init_py = true;
            }
            entries.push(name);
        }
        entries.sort();
    }
    ImportlibResourcesPathPayload {
        basename,
        exists,
        is_file,
        is_dir,
        entries,
        has_init_py,
        is_archive_member: false,
    }
}

fn importlib_resources_package_payload(
    package: &str,
    search_paths: &[String],
    module_file: Option<String>,
) -> ImportlibResourcesPackagePayload {
    let resolved = importlib_search_paths(search_paths, module_file.clone());
    let resolution = importlib_find_in_path(package, &resolved, false);
    let mut roots: Vec<String> = Vec::new();
    let mut has_regular_package = false;
    let mut init_file: Option<String> = None;
    if let Some(spec) = resolution {
        if spec.is_package
            && matches!(
                spec.loader_kind.as_str(),
                "source" | "zip_source" | "bytecode"
            )
        {
            has_regular_package = true;
            init_file = spec.origin.clone();
            if let Some(locations) = spec.submodule_search_locations {
                for location in locations {
                    append_unique_path(&mut roots, &location);
                }
            } else {
                let sep = bootstrap_path_sep();
                if let Some(origin) = spec.origin.as_deref() {
                    let dir = path_dirname_text(origin, sep);
                    if !dir.is_empty() {
                        append_unique_path(&mut roots, &dir);
                    }
                }
            }
        } else if spec.is_package
            && spec.loader_kind == "namespace"
            && let Some(locations) = spec.submodule_search_locations
        {
            for location in locations {
                append_unique_path(&mut roots, &location);
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

fn importlib_resources_open_resource_bytes_from_package_impl(
    _py: &PyToken<'_>,
    package: &str,
    search_paths: &[String],
    module_file: Option<String>,
    resource: &str,
) -> Result<Vec<u8>, u64> {
    let payload =
        importlib_resources_required_package_payload(_py, package, search_paths, module_file)?;
    importlib_validate_resource_name_text(_py, resource)?;
    if let Some(candidate) = importlib_resources_first_file_candidate(&payload.roots, resource) {
        return importlib_read_file_bytes(_py, &candidate);
    }
    Err(raise_exception::<_>(_py, "FileNotFoundError", resource))
}

fn importlib_resources_join_parts_path(parts: &[String]) -> String {
    let sep = bootstrap_path_sep();
    let mut out = String::new();
    for part in parts {
        out = path_join_text(out, part, sep);
    }
    out
}

fn importlib_resources_name_parts(name: &str) -> Vec<String> {
    let normalized = name.replace('\\', "/");
    normalized
        .split('/')
        .filter(|part| !part.is_empty())
        .map(|part| part.to_string())
        .collect()
}

fn importlib_resources_candidate_path(root: &str, resource: &str) -> String {
    if resource.is_empty() {
        root.to_string()
    } else {
        let sep = bootstrap_path_sep();
        path_join_text(root.to_string(), resource, sep)
    }
}

fn importlib_resources_open_resource_bytes_from_package_parts_impl(
    _py: &PyToken<'_>,
    package: &str,
    search_paths: &[String],
    module_file: Option<String>,
    path_parts: &[String],
) -> Result<Vec<u8>, u64> {
    let payload =
        importlib_resources_required_package_payload(_py, package, search_paths, module_file)?;
    let resource = importlib_resources_join_parts_path(path_parts);
    let mut first_dir: Option<String> = None;
    for root in &payload.roots {
        let candidate = importlib_resources_candidate_path(root, &resource);
        let payload = importlib_resources_path_payload(&candidate);
        if payload.is_file {
            return importlib_read_file_bytes(_py, &candidate);
        }
        if payload.is_dir && first_dir.is_none() {
            first_dir = Some(candidate);
        }
    }
    if let Some(path) = first_dir {
        return Err(raise_exception::<_>(_py, "IsADirectoryError", &path));
    }
    let not_found = if resource.is_empty() {
        package
    } else {
        resource.as_str()
    };
    Err(raise_exception::<_>(_py, "FileNotFoundError", not_found))
}

fn importlib_resources_is_resource_from_package_parts_impl(
    _py: &PyToken<'_>,
    package: &str,
    search_paths: &[String],
    module_file: Option<String>,
    path_parts: &[String],
) -> Result<bool, u64> {
    if path_parts.is_empty() {
        return Ok(false);
    }
    let payload =
        importlib_resources_required_package_payload(_py, package, search_paths, module_file)?;
    let resource = importlib_resources_join_parts_path(path_parts);
    for root in &payload.roots {
        let candidate = importlib_resources_candidate_path(root, &resource);
        let payload = importlib_resources_path_payload(&candidate);
        if payload.is_file {
            return Ok(true);
        }
    }
    Ok(false)
}

fn importlib_resources_contents_from_package_parts_impl(
    _py: &PyToken<'_>,
    package: &str,
    search_paths: &[String],
    module_file: Option<String>,
    path_parts: &[String],
) -> Result<Vec<String>, u64> {
    let payload =
        importlib_resources_required_package_payload(_py, package, search_paths, module_file)?;
    let resource = importlib_resources_join_parts_path(path_parts);
    let mut entries: BTreeSet<String> = BTreeSet::new();
    let mut has_init_py = false;
    for root in &payload.roots {
        let candidate = importlib_resources_candidate_path(root, &resource);
        let payload = importlib_resources_path_payload(&candidate);
        if !payload.is_dir {
            continue;
        }
        has_init_py = has_init_py || payload.has_init_py;
        for entry in payload.entries {
            entries.insert(entry);
        }
    }
    if has_init_py {
        entries.insert(String::from("__pycache__"));
    }
    Ok(entries.into_iter().collect())
}

fn importlib_resources_resource_path_from_package_parts_impl(
    _py: &PyToken<'_>,
    package: &str,
    search_paths: &[String],
    module_file: Option<String>,
    path_parts: &[String],
) -> Result<Option<String>, u64> {
    let payload =
        importlib_resources_required_package_payload(_py, package, search_paths, module_file)?;
    let resource = importlib_resources_join_parts_path(path_parts);
    for root in &payload.roots {
        let candidate = importlib_resources_candidate_path(root, &resource);
        let payload = importlib_resources_path_payload(&candidate);
        if payload.exists && !payload.is_archive_member {
            return Ok(Some(candidate));
        }
    }
    Ok(None)
}

fn importlib_resources_required_package_payload(
    _py: &PyToken<'_>,
    package: &str,
    search_paths: &[String],
    module_file: Option<String>,
) -> Result<ImportlibResourcesPackagePayload, u64> {
    let payload = importlib_resources_package_payload(package, search_paths, module_file);
    if payload.roots.is_empty() {
        return Err(raise_exception::<_>(_py, "ModuleNotFoundError", package));
    }
    Ok(payload)
}

fn importlib_resources_first_file_candidate(roots: &[String], resource: &str) -> Option<String> {
    let sep = bootstrap_path_sep();
    for root in roots {
        let candidate = path_join_text(root.clone(), resource, sep);
        let payload = importlib_resources_path_payload(&candidate);
        if payload.is_file {
            return Some(candidate);
        }
    }
    None
}

fn importlib_resources_files_payload(
    _py: &PyToken<'_>,
    module_bits: u64,
    fallback_bits: u64,
    search_paths: &[String],
    module_file: Option<String>,
) -> Result<ImportlibResourcesFilesPayload, u64> {
    let package_name = importlib_resources_module_name_from_bits(_py, module_bits, fallback_bits)?;
    let package_payload =
        importlib_resources_package_payload(&package_name, search_paths, module_file);
    let reader_bits = importlib_resources_loader_reader_from_bits(_py, module_bits, &package_name)?;
    let mut roots = package_payload.roots;
    let mut is_namespace = package_payload.is_namespace;
    let mut files_traversable_bits: Option<u64> = None;
    if let Some(reader_bits_value) = reader_bits {
        files_traversable_bits = importlib_reader_files_traversable_bits(_py, reader_bits_value)?;
        let reader_roots = importlib_resources_reader_roots_impl(_py, reader_bits_value)?;
        if !reader_roots.is_empty() {
            roots = reader_roots;
            is_namespace = package_payload.is_namespace && roots.len() > 1;
        }
    }
    Ok(ImportlibResourcesFilesPayload {
        package_name,
        roots,
        is_namespace,
        reader_bits,
        files_traversable_bits,
    })
}

fn importlib_resources_first_fs_file_candidate(roots: &[String], resource: &str) -> Option<String> {
    let sep = bootstrap_path_sep();
    for root in roots {
        let candidate = path_join_text(root.clone(), resource, sep);
        let payload = importlib_resources_path_payload(&candidate);
        if payload.is_file && !payload.is_archive_member {
            return Some(candidate);
        }
    }
    None
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

fn importlib_metadata_parse_csv_row(row: &str) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut chars = row.chars().peekable();
    let mut in_quotes = false;
    while let Some(ch) = chars.next() {
        if in_quotes {
            if ch == '"' {
                if chars.peek().is_some_and(|next| *next == '"') {
                    current.push('"');
                    let _ = chars.next();
                } else {
                    in_quotes = false;
                }
            } else {
                current.push(ch);
            }
            continue;
        }
        match ch {
            ',' => {
                out.push(current);
                current = String::new();
            }
            '"' => in_quotes = true,
            _ => current.push(ch),
        }
    }
    out.push(current);
    out
}

fn importlib_metadata_record_payload(path: &str) -> Vec<ImportlibMetadataRecordEntry> {
    let sep = bootstrap_path_sep();
    let record_path = path_join_text(path.to_string(), "RECORD", sep);
    let record_text = match std::fs::read(&record_path) {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(_) => return Vec::new(),
    };
    let mut out: Vec<ImportlibMetadataRecordEntry> = Vec::new();
    for raw_line in record_text.lines() {
        let line = raw_line.trim_end_matches('\r');
        if line.trim().is_empty() {
            continue;
        }
        let fields = importlib_metadata_parse_csv_row(line);
        let Some(path_field) = fields.first() else {
            continue;
        };
        let rel_path = path_field.trim();
        if rel_path.is_empty() {
            continue;
        }
        let hash = fields
            .get(1)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        let size = fields
            .get(2)
            .map(|value| value.trim().to_string())
            .filter(|value| !value.is_empty());
        out.push(ImportlibMetadataRecordEntry {
            path: rel_path.to_string(),
            hash,
            size,
        });
    }
    out
}

fn importlib_metadata_packages_distributions_payload(
    search_paths: &[String],
    module_file: Option<String>,
) -> Vec<(String, Vec<String>)> {
    let mut mapping: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    let sep = bootstrap_path_sep();
    let dist_paths = importlib_metadata_dist_paths(search_paths, module_file);
    for path in dist_paths {
        let payload = importlib_metadata_payload(&path);
        let dist_name = payload.name.trim().to_string();
        if dist_name.is_empty() {
            continue;
        }
        let top_level_path = path_join_text(path, "top_level.txt", sep);
        let top_level_text = match std::fs::read(&top_level_path) {
            Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
            Err(_) => continue,
        };
        for line in top_level_text.lines() {
            let package = line.trim();
            if package.is_empty() {
                continue;
            }
            mapping
                .entry(package.to_string())
                .or_default()
                .insert(dist_name.clone());
        }
    }
    mapping
        .into_iter()
        .map(|(package, providers)| (package, providers.into_iter().collect()))
        .collect()
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

fn importlib_normalize_path_text(path: &str) -> String {
    path.replace('\\', "/")
}

fn importlib_is_archive_member_path(path: &str) -> bool {
    importlib_normalize_path_text(path).contains(".zip/")
}

fn importlib_package_root_from_origin(path: &str) -> Option<String> {
    let normalized = importlib_normalize_path_text(path);
    if normalized.ends_with("/__init__.py") || normalized.ends_with("/__init__.pyc") {
        return normalized
            .rsplit_once('/')
            .map(|(root, _)| root.to_string());
    }
    None
}

fn importlib_validate_resource_name_text(_py: &PyToken<'_>, resource: &str) -> Result<(), u64> {
    if resource.is_empty() || resource == "." || resource == ".." {
        return Err(raise_exception::<_>(
            _py,
            "ValueError",
            &format!(
                "'{}' must be only a file name",
                resource.replace('\'', "\\'")
            ),
        ));
    }
    if resource.contains('/') || resource.contains('\\') {
        return Err(raise_exception::<_>(
            _py,
            "ValueError",
            &format!(
                "'{}' must be only a file name",
                resource.replace('\'', "\\'")
            ),
        ));
    }
    Ok(())
}

fn importlib_iter_next_value_bits(_py: &PyToken<'_>, iter_bits: u64) -> Result<Option<u64>, u64> {
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
        return Ok(None);
    }
    inc_ref_bits(_py, pair[0]);
    Ok(Some(pair[0]))
}

fn importlib_best_effort_str(_py: &PyToken<'_>, value_bits: u64) -> String {
    let text_bits = unsafe { call_callable1(_py, builtin_classes(_py).str, value_bits) };
    if exception_pending(_py) {
        clear_exception(_py);
        return String::from("<object>");
    }
    let out =
        string_obj_to_owned(obj_from_bits(text_bits)).unwrap_or_else(|| String::from("<object>"));
    if !obj_from_bits(text_bits).is_none() {
        dec_ref_bits(_py, text_bits);
    }
    out
}

fn importlib_exception_name_from_bits(_py: &PyToken<'_>, class_bits: u64) -> Option<String> {
    static NAME_NAME: AtomicU64 = AtomicU64::new(0);
    let name_attr = intern_static_name(_py, &NAME_NAME, b"__name__");
    let missing = missing_bits(_py);
    let name_bits = molt_getattr_builtin(class_bits, name_attr, missing);
    if exception_pending(_py) {
        clear_exception(_py);
        return None;
    }
    if is_missing_bits(_py, name_bits) {
        if !obj_from_bits(name_bits).is_none() {
            dec_ref_bits(_py, name_bits);
        }
        return None;
    }
    let name = string_obj_to_owned(obj_from_bits(name_bits));
    if !obj_from_bits(name_bits).is_none() {
        dec_ref_bits(_py, name_bits);
    }
    name
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

fn importlib_read_file_bytes(_py: &PyToken<'_>, path: &str) -> Result<Vec<u8>, u64> {
    if let Some((archive_path, inner_path)) = split_zip_archive_path(path) {
        let archive_exists = match std::fs::metadata(&archive_path) {
            Ok(metadata) => metadata.is_file(),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
            Err(err) => return Err(raise_importlib_io_error(_py, err)),
        };
        if archive_exists {
            if inner_path.is_empty() {
                return Err(raise_importlib_io_error(
                    _py,
                    std::io::Error::new(
                        std::io::ErrorKind::IsADirectory,
                        "zip archive root is a directory",
                    ),
                ));
            }
            return match zip_archive_read_entry(&archive_path, &inner_path) {
                Ok(bytes) => Ok(bytes),
                Err(err) => {
                    if zip_archive_has_prefix(&archive_path, &inner_path) {
                        Err(raise_importlib_io_error(
                            _py,
                            std::io::Error::new(std::io::ErrorKind::IsADirectory, err.to_string()),
                        ))
                    } else {
                        Err(raise_importlib_io_error(_py, err))
                    }
                }
            };
        }
    }
    std::fs::read(path).map_err(|err| raise_importlib_io_error(_py, err))
}

fn importlib_path_is_file(_py: &PyToken<'_>, path: &str) -> Result<bool, u64> {
    if let Some((archive_path, inner_path)) = split_zip_archive_path(path) {
        let archive_exists = match std::fs::metadata(&archive_path) {
            Ok(metadata) => metadata.is_file(),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => false,
            Err(err) => return Err(raise_importlib_io_error(_py, err)),
        };
        if archive_exists {
            if inner_path.is_empty() {
                return Ok(false);
            }
            if zip_archive_entry_exists(&archive_path, &inner_path) {
                return Ok(true);
            }
            return Ok(false);
        }
    }
    match std::fs::metadata(path) {
        Ok(metadata) => Ok(metadata.is_file()),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(err) => Err(raise_importlib_io_error(_py, err)),
    }
}

struct LoadedExtensionManifest {
    source: String,
    manifest: JsonValue,
    wheel_path: Option<String>,
}

fn importlib_sha256_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = Sha256::digest(bytes);
    let mut out = String::with_capacity(digest.len() * 2);
    for byte in digest {
        out.push(HEX[(byte >> 4) as usize] as char);
        out.push(HEX[(byte & 0x0F) as usize] as char);
    }
    out
}

fn importlib_sha256_file(path: &str) -> Result<String, std::io::Error> {
    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    loop {
        let read = file.read(&mut buf)?;
        if read == 0 {
            break;
        }
        hasher.update(&buf[..read]);
    }
    let digest = hasher.finalize();
    Ok(importlib_sha256_hex(digest.as_ref()))
}

fn importlib_metadata_timestamp_nanos(meta: &std::fs::Metadata) -> u128 {
    meta.modified()
        .ok()
        .and_then(|value| value.duration_since(UNIX_EPOCH).ok())
        .map(|value| value.as_nanos())
        .unwrap_or(0)
}

fn importlib_metadata_fingerprint(path: &str) -> Result<String, std::io::Error> {
    let metadata = std::fs::metadata(path)?;
    let kind = if metadata.is_file() {
        "f"
    } else if metadata.is_dir() {
        "d"
    } else {
        "o"
    };
    Ok(format!(
        "{kind}:{}:{}",
        metadata.len(),
        importlib_metadata_timestamp_nanos(&metadata)
    ))
}

fn importlib_cache_fingerprint_for_path(path: &str) -> Result<String, std::io::Error> {
    if let Some((archive_path, inner_path)) = split_zip_archive_path(path) {
        let archive_fp = importlib_metadata_fingerprint(&archive_path)?;
        return Ok(format!("zip:{archive_fp}:{inner_path}"));
    }
    importlib_metadata_fingerprint(path).map(|fp| format!("file:{fp}"))
}

fn importlib_manifest_cache_fingerprint(
    loaded: &LoadedExtensionManifest,
) -> Result<String, std::io::Error> {
    if let Some(wheel_path) = loaded.wheel_path.as_deref() {
        let wheel_fp = importlib_metadata_fingerprint(wheel_path)?;
        return Ok(format!("wheel:{wheel_fp}:{}", loaded.source));
    }
    let sidecar_fp = importlib_metadata_fingerprint(loaded.source.as_str())?;
    Ok(format!("sidecar:{sidecar_fp}:{}", loaded.source))
}

fn importlib_sha256_path(_py: &PyToken<'_>, path: &str) -> Result<String, u64> {
    if let Some((archive_path, inner_path)) = split_zip_archive_path(path) {
        let bytes = zip_archive_read_entry(&archive_path, &inner_path)
            .map_err(|err| raise_importlib_io_error(_py, err))?;
        return Ok(importlib_sha256_hex(&bytes));
    }
    importlib_sha256_file(path).map_err(|err| raise_importlib_io_error(_py, err))
}

fn importlib_normalize_path_separators(path: &str) -> String {
    path.replace('\\', "/")
}

fn importlib_extension_path_matches_manifest(path: &str, manifest_extension: &str) -> bool {
    let path_norm = importlib_normalize_path_separators(path);
    let expected_norm = importlib_normalize_path_separators(manifest_extension)
        .trim_matches('/')
        .to_string();
    if expected_norm.is_empty() {
        return false;
    }
    if path_norm == expected_norm || path_norm.ends_with(&format!("/{expected_norm}")) {
        return true;
    }
    let path_name = Path::new(path_norm.as_str())
        .file_name()
        .and_then(|value| value.to_str());
    let expected_name = Path::new(expected_norm.as_str())
        .file_name()
        .and_then(|value| value.to_str());
    path_name.is_some() && path_name == expected_name
}

fn importlib_find_extension_manifest_sidecar(path: &str) -> Result<Option<String>, std::io::Error> {
    let mut current = Path::new(path).parent();
    for _ in 0..6 {
        let Some(dir) = current else {
            break;
        };
        let candidate = dir.join("extension_manifest.json");
        match std::fs::metadata(&candidate) {
            Ok(meta) => {
                if meta.is_file() {
                    return Ok(Some(candidate.to_string_lossy().into_owned()));
                }
            }
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {}
            Err(err) => return Err(err),
        }
        current = dir.parent();
    }
    Ok(None)
}

fn importlib_load_extension_manifest_for_path(
    _py: &PyToken<'_>,
    path: &str,
) -> Result<LoadedExtensionManifest, u64> {
    if let Some((archive_path, _inner_path)) = split_zip_archive_path(path) {
        let manifest_bytes = match zip_archive_read_entry(&archive_path, "extension_manifest.json")
        {
            Ok(bytes) => bytes,
            Err(err) => {
                return Err(raise_exception::<_>(
                    _py,
                    "ImportError",
                    &format!(
                        "extension metadata missing for {path:?}: unable to read extension_manifest.json ({err})"
                    ),
                ));
            }
        };
        let manifest = match serde_json::from_slice::<JsonValue>(&manifest_bytes) {
            Ok(decoded) => decoded,
            Err(err) => {
                return Err(raise_exception::<_>(
                    _py,
                    "ImportError",
                    &format!(
                        "invalid extension metadata in {}: {err}",
                        format!("{archive_path}/extension_manifest.json")
                    ),
                ));
            }
        };
        return Ok(LoadedExtensionManifest {
            source: format!("{archive_path}/extension_manifest.json"),
            manifest,
            wheel_path: Some(archive_path),
        });
    }

    let Some(manifest_path) = importlib_find_extension_manifest_sidecar(path)
        .map_err(|err| raise_importlib_io_error(_py, err))?
    else {
        return Err(raise_exception::<_>(
            _py,
            "ImportError",
            &format!(
                "extension metadata missing for {path:?}: extension_manifest.json not found near extension path"
            ),
        ));
    };
    let manifest_bytes =
        std::fs::read(&manifest_path).map_err(|err| raise_importlib_io_error(_py, err))?;
    let manifest = serde_json::from_slice::<JsonValue>(&manifest_bytes).map_err(|err| {
        raise_exception::<u64>(
            _py,
            "ImportError",
            &format!("invalid extension metadata in {manifest_path}: {err}"),
        )
    })?;
    Ok(LoadedExtensionManifest {
        source: manifest_path,
        manifest,
        wheel_path: None,
    })
}

fn importlib_manifest_required_string<'a>(
    _py: &PyToken<'_>,
    manifest: &'a serde_json::Map<String, JsonValue>,
    field: &str,
    manifest_source: &str,
) -> Result<&'a str, u64> {
    let value = manifest
        .get(field)
        .and_then(JsonValue::as_str)
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| {
            raise_exception::<u64>(
                _py,
                "ImportError",
                &format!(
                    "invalid extension metadata {manifest_source}: missing or invalid field {field:?}"
                ),
            )
        })?;
    Ok(value)
}

fn importlib_validate_extension_metadata(
    _py: &PyToken<'_>,
    module_name: &str,
    path: &str,
    extension_sha256: &str,
    loaded: &LoadedExtensionManifest,
) -> Result<(), u64> {
    let Some(manifest) = loaded.manifest.as_object() else {
        return Err(raise_exception::<_>(
            _py,
            "ImportError",
            &format!(
                "invalid extension metadata {}: payload must be a JSON object",
                loaded.source
            ),
        ));
    };

    let manifest_module =
        importlib_manifest_required_string(_py, manifest, "module", loaded.source.as_str())?;
    if manifest_module != module_name {
        return Err(raise_exception::<_>(
            _py,
            "ImportError",
            &format!(
                "extension metadata module mismatch in {}: manifest has {:?}, loader requested {:?}",
                loaded.source, manifest_module, module_name
            ),
        ));
    }

    let abi_version = importlib_manifest_required_string(
        _py,
        manifest,
        "molt_c_api_version",
        loaded.source.as_str(),
    )?;
    let manifest_abi_major = abi_version
        .split('.')
        .next()
        .and_then(|segment| segment.parse::<u32>().ok())
        .ok_or_else(|| {
            raise_exception::<u64>(
                _py,
                "ImportError",
                &format!(
                    "invalid extension ABI version in {}: {:?}",
                    loaded.source, abi_version
                ),
            )
        })?;
    let runtime_abi_major = crate::c_api::MOLT_C_API_VERSION;
    if manifest_abi_major != runtime_abi_major {
        return Err(raise_exception::<_>(
            _py,
            "ImportError",
            &format!(
                "extension ABI mismatch for {:?}: runtime requires {}, manifest declares {} (source: {})",
                module_name, runtime_abi_major, manifest_abi_major, loaded.source
            ),
        ));
    }

    let abi_tag =
        importlib_manifest_required_string(_py, manifest, "abi_tag", loaded.source.as_str())?;
    let expected_abi_tag = format!("molt_abi{manifest_abi_major}");
    if abi_tag != expected_abi_tag {
        return Err(raise_exception::<_>(
            _py,
            "ImportError",
            &format!(
                "extension ABI tag mismatch in {}: expected {expected_abi_tag}, found {abi_tag}",
                loaded.source
            ),
        ));
    }

    let _target_triple =
        importlib_manifest_required_string(_py, manifest, "target_triple", loaded.source.as_str())?;
    let _platform_tag =
        importlib_manifest_required_string(_py, manifest, "platform_tag", loaded.source.as_str())?;

    let manifest_extension =
        importlib_manifest_required_string(_py, manifest, "extension", loaded.source.as_str())?;
    if !importlib_extension_path_matches_manifest(path, manifest_extension) {
        return Err(raise_exception::<_>(
            _py,
            "ImportError",
            &format!(
                "extension metadata path mismatch in {}: manifest expects {:?}, loader path is {:?}",
                loaded.source, manifest_extension, path
            ),
        ));
    }

    let expected_extension_sha = importlib_manifest_required_string(
        _py,
        manifest,
        "extension_sha256",
        loaded.source.as_str(),
    )?
    .to_ascii_lowercase();
    if expected_extension_sha != extension_sha256 {
        return Err(raise_exception::<_>(
            _py,
            "ImportError",
            &format!(
                "extension checksum mismatch for {:?}: expected {}, got {}",
                module_name, expected_extension_sha, extension_sha256
            ),
        ));
    }

    if let Some(wheel_path) = loaded.wheel_path.as_deref() {
        let expected_wheel_sha = importlib_manifest_required_string(
            _py,
            manifest,
            "wheel_sha256",
            loaded.source.as_str(),
        )?
        .to_ascii_lowercase();
        let actual_wheel_sha =
            importlib_sha256_file(wheel_path).map_err(|err| raise_importlib_io_error(_py, err))?;
        if expected_wheel_sha != actual_wheel_sha {
            return Err(raise_exception::<_>(
                _py,
                "ImportError",
                &format!(
                    "wheel checksum mismatch for extension metadata {}: expected {}, got {}",
                    loaded.source, expected_wheel_sha, actual_wheel_sha
                ),
            ));
        }

        if let Some(wheel_name) = manifest
            .get("wheel")
            .and_then(JsonValue::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            let archive_name = Path::new(wheel_path)
                .file_name()
                .and_then(|value| value.to_str())
                .unwrap_or_default();
            if archive_name != wheel_name {
                return Err(raise_exception::<_>(
                    _py,
                    "ImportError",
                    &format!(
                        "wheel name mismatch in {}: manifest has {:?}, archive is {:?}",
                        loaded.source, wheel_name, archive_name
                    ),
                ));
            }
        }
    }

    let capabilities = manifest
        .get("capabilities")
        .and_then(JsonValue::as_array)
        .ok_or_else(|| {
            raise_exception::<u64>(
                _py,
                "ImportError",
                &format!(
                    "invalid extension metadata {}: capabilities must be an array",
                    loaded.source
                ),
            )
        })?;
    if capabilities.is_empty() {
        return Err(raise_exception::<_>(
            _py,
            "ImportError",
            &format!(
                "invalid extension metadata {}: capabilities list must not be empty",
                loaded.source
            ),
        ));
    }
    let mut missing_caps: BTreeSet<String> = BTreeSet::new();
    for cap_value in capabilities {
        let Some(cap_raw) = cap_value.as_str() else {
            return Err(raise_exception::<_>(
                _py,
                "ImportError",
                &format!(
                    "invalid extension metadata {}: capabilities must contain only strings",
                    loaded.source
                ),
            ));
        };
        let cap = cap_raw.trim();
        if cap.is_empty() {
            return Err(raise_exception::<_>(
                _py,
                "ImportError",
                &format!(
                    "invalid extension metadata {}: capability entries must be non-empty",
                    loaded.source
                ),
            ));
        }
        if !has_capability(_py, cap) {
            missing_caps.insert(cap.to_string());
        }
    }
    if !missing_caps.is_empty() {
        return Err(raise_exception::<_>(
            _py,
            "PermissionError",
            &format!(
                "missing extension capabilities for {:?}: {}",
                module_name,
                missing_caps.into_iter().collect::<Vec<_>>().join(", ")
            ),
        ));
    }

    Ok(())
}

fn importlib_require_extension_metadata(
    _py: &PyToken<'_>,
    module_name: &str,
    path: &str,
) -> Result<(), u64> {
    let cache_key = format!("{module_name}\u{1f}{path}");
    let loaded = importlib_load_extension_manifest_for_path(_py, path)?;
    let path_fingerprint = importlib_cache_fingerprint_for_path(path)
        .map_err(|err| raise_importlib_io_error(_py, err))?;
    let manifest_fingerprint = importlib_manifest_cache_fingerprint(&loaded)
        .map_err(|err| raise_importlib_io_error(_py, err))?;
    let cache_value = format!("{path_fingerprint}\u{1f}{manifest_fingerprint}");
    {
        let cache = extension_metadata_ok_cache();
        let guard = cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if guard
            .get(&cache_key)
            .is_some_and(|value| value == &cache_value)
        {
            return Ok(());
        }
    }

    let extension_sha256 = importlib_sha256_path(_py, path)?;
    importlib_validate_extension_metadata(_py, module_name, path, &extension_sha256, &loaded)?;

    let cache = extension_metadata_ok_cache();
    let mut guard = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.insert(cache_key, cache_value);
    Ok(())
}

fn importlib_extension_exec_unavailable(
    _py: &PyToken<'_>,
    module_name: &str,
    path: &str,
    kind: &str,
    shim_candidates: &[String],
) -> u64 {
    let mut message = format!(
        "libmolt {kind} execution has no intrinsic execution candidate for {module_name:?} at {path:?}"
    );
    if !shim_candidates.is_empty() {
        message.push_str(" (searched intrinsic execution candidates: ");
        message.push_str(&shim_candidates.join(", "));
        message.push(')');
    }
    raise_exception::<u64>(_py, "ImportError", message.as_str())
}

fn importlib_decode_source_text(source_bytes: &[u8]) -> String {
    match crate::object::ops::decode_bytes_text("utf-8", "surrogateescape", source_bytes) {
        Ok((text, _encoding)) => String::from_utf8_lossy(&text).into_owned(),
        Err(_) => String::from_utf8_lossy(source_bytes).into_owned(),
    }
}

fn importlib_exec_restricted_source_path(
    _py: &PyToken<'_>,
    namespace_ptr: *mut u8,
    source_path: &str,
) -> Result<(), u64> {
    let source_bytes = importlib_read_file_bytes(_py, source_path)?;
    let source = importlib_decode_source_text(&source_bytes);
    // NOTE(dynamic-exec-policy): Restricted source shim execution is intentional
    // for compiled binaries. Native extension/pyc execution parity is deferred
    // until an explicit capability-gated design is approved with perf evidence.
    unsafe { runpy_exec_restricted_source(_py, namespace_ptr, &source, source_path) }
}

fn importlib_restricted_exec_error_message(
    _py: &PyToken<'_>,
    kind: &str,
    module_name: &str,
    source_path: &str,
) -> Option<String> {
    if clear_pending_if_kind(_py, &["NotImplementedError"]) {
        let message = format!(
            "unsupported {kind} shim semantics for {module_name:?} at {source_path:?}; \
restricted source execution only supports docstring/pass/import/from-import/literal-assignment payloads"
        );
        return Some(message);
    }
    None
}

fn linecache_loader_get_source_impl(
    _py: &PyToken<'_>,
    loader_bits: u64,
    module_name: &str,
) -> Result<Option<String>, u64> {
    static GET_SOURCE_NAME: AtomicU64 = AtomicU64::new(0);
    let get_source_name = intern_static_name(_py, &GET_SOURCE_NAME, b"get_source");
    let Some(call_bits) = importlib_reader_lookup_callable(_py, loader_bits, get_source_name)?
    else {
        return Ok(None);
    };
    let module_name_bits = alloc_str_bits(_py, module_name)?;
    let value_bits = unsafe { call_callable1(_py, call_bits, module_name_bits) };
    dec_ref_bits(_py, module_name_bits);
    dec_ref_bits(_py, call_bits);
    if exception_pending(_py) {
        if clear_pending_if_kind(_py, &["ImportError", "OSError"]) {
            return Ok(None);
        }
        return Err(MoltObject::none().bits());
    }
    if obj_from_bits(value_bits).is_none() {
        return Ok(None);
    }
    let Some(text) = string_obj_to_owned(obj_from_bits(value_bits)) else {
        if !obj_from_bits(value_bits).is_none() {
            dec_ref_bits(_py, value_bits);
        }
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            "loader.get_source() must return str or None",
        ));
    };
    if !obj_from_bits(value_bits).is_none() {
        dec_ref_bits(_py, value_bits);
    }
    Ok(Some(text))
}

fn importlib_reader_lookup_callable(
    _py: &PyToken<'_>,
    target_bits: u64,
    name_bits: u64,
) -> Result<Option<u64>, u64> {
    let missing = missing_bits(_py);
    let attr_bits = molt_getattr_builtin(target_bits, name_bits, missing);
    if exception_pending(_py) {
        if clear_pending_if_kind(_py, &["AttributeError"]) {
            return Ok(None);
        }
        return Err(MoltObject::none().bits());
    }
    if is_missing_bits(_py, attr_bits) {
        return Ok(None);
    }
    let callable_bits = molt_is_callable(attr_bits);
    let is_callable = is_truthy(_py, obj_from_bits(callable_bits));
    if !obj_from_bits(callable_bits).is_none() {
        dec_ref_bits(_py, callable_bits);
    }
    if !is_callable {
        if !obj_from_bits(attr_bits).is_none() {
            dec_ref_bits(_py, attr_bits);
        }
        return Ok(None);
    }
    Ok(Some(attr_bits))
}

fn getattr_optional_bits(
    _py: &PyToken<'_>,
    target_bits: u64,
    name_bits: u64,
) -> Result<Option<u64>, u64> {
    let missing = missing_bits(_py);
    let attr_bits = molt_getattr_builtin(target_bits, name_bits, missing);
    if exception_pending(_py) {
        if clear_pending_if_kind(_py, &["AttributeError"]) {
            return Ok(None);
        }
        return Err(MoltObject::none().bits());
    }
    if is_missing_bits(_py, attr_bits) {
        return Ok(None);
    }
    Ok(Some(attr_bits))
}

fn importlib_module_spec_is_package_bits(_py: &PyToken<'_>, module_bits: u64) -> Result<bool, u64> {
    static SPEC_NAME: AtomicU64 = AtomicU64::new(0);
    static SUBMODULE_SEARCH_LOCATIONS_NAME: AtomicU64 = AtomicU64::new(0);
    let spec_name = intern_static_name(_py, &SPEC_NAME, b"__spec__");
    let Some(spec_bits) = getattr_optional_bits(_py, module_bits, spec_name)? else {
        return Ok(false);
    };
    let submodule_search_locations_name = intern_static_name(
        _py,
        &SUBMODULE_SEARCH_LOCATIONS_NAME,
        b"submodule_search_locations",
    );
    let locations_bits = getattr_optional_bits(_py, spec_bits, submodule_search_locations_name)?;
    if !obj_from_bits(spec_bits).is_none() {
        dec_ref_bits(_py, spec_bits);
    }
    let Some(locations_bits) = locations_bits else {
        return Ok(false);
    };
    let is_package = !obj_from_bits(locations_bits).is_none();
    if !obj_from_bits(locations_bits).is_none() {
        dec_ref_bits(_py, locations_bits);
    }
    Ok(is_package)
}

fn traceback_exception_suppress_context_bits(
    _py: &PyToken<'_>,
    value_bits: u64,
) -> Result<bool, u64> {
    static SUPPRESS_CONTEXT_NAME: AtomicU64 = AtomicU64::new(0);
    if obj_from_bits(value_bits).is_none() {
        return Ok(false);
    }
    let suppress_context_name =
        intern_static_name(_py, &SUPPRESS_CONTEXT_NAME, b"__suppress_context__");
    let Some(suppress_bits) = getattr_optional_bits(_py, value_bits, suppress_context_name)? else {
        return Ok(false);
    };
    let out = is_truthy(_py, obj_from_bits(suppress_bits));
    if !obj_from_bits(suppress_bits).is_none() {
        dec_ref_bits(_py, suppress_bits);
    }
    Ok(out)
}

fn importlib_resources_module_name_from_bits(
    _py: &PyToken<'_>,
    module_bits: u64,
    fallback_bits: u64,
) -> Result<String, u64> {
    static NAME_NAME: AtomicU64 = AtomicU64::new(0);
    static SPEC_NAME: AtomicU64 = AtomicU64::new(0);
    static PACKAGE_NAME: AtomicU64 = AtomicU64::new(0);

    let module_name_name = intern_static_name(_py, &NAME_NAME, b"__name__");
    if let Some(name_bits) = getattr_optional_bits(_py, module_bits, module_name_name)? {
        let out = string_obj_to_owned(obj_from_bits(name_bits));
        if !obj_from_bits(name_bits).is_none() {
            dec_ref_bits(_py, name_bits);
        }
        if let Some(name) = out
            && !name.is_empty()
        {
            return Ok(name);
        }
    }

    if !obj_from_bits(fallback_bits).is_none() {
        let Some(fallback) = string_obj_to_owned(obj_from_bits(fallback_bits)) else {
            return Err(raise_exception::<_>(
                _py,
                "TypeError",
                "fallback must be str or None",
            ));
        };
        if !fallback.is_empty() {
            return Ok(fallback);
        }
    }

    let spec_name = intern_static_name(_py, &SPEC_NAME, b"__spec__");
    if let Some(spec_bits) = getattr_optional_bits(_py, module_bits, spec_name)? {
        if let Some(spec_mod_name_bits) = getattr_optional_bits(_py, spec_bits, module_name_name)? {
            let out = string_obj_to_owned(obj_from_bits(spec_mod_name_bits));
            if !obj_from_bits(spec_mod_name_bits).is_none() {
                dec_ref_bits(_py, spec_mod_name_bits);
            }
            if !obj_from_bits(spec_bits).is_none() {
                dec_ref_bits(_py, spec_bits);
            }
            if let Some(name) = out
                && !name.is_empty()
            {
                return Ok(name);
            }
        } else if !obj_from_bits(spec_bits).is_none() {
            dec_ref_bits(_py, spec_bits);
        }
    }

    let package_name = intern_static_name(_py, &PACKAGE_NAME, b"__package__");
    if let Some(package_bits) = getattr_optional_bits(_py, module_bits, package_name)? {
        let out = string_obj_to_owned(obj_from_bits(package_bits));
        if !obj_from_bits(package_bits).is_none() {
            dec_ref_bits(_py, package_bits);
        }
        if let Some(name) = out
            && !name.is_empty()
        {
            return Ok(name);
        }
    }

    Err(raise_exception::<_>(_py, "ModuleNotFoundError", "unknown"))
}

fn importlib_resources_loader_reader_from_bits(
    _py: &PyToken<'_>,
    module_bits: u64,
    module_name: &str,
) -> Result<Option<u64>, u64> {
    static SPEC_NAME: AtomicU64 = AtomicU64::new(0);
    static LOADER_NAME: AtomicU64 = AtomicU64::new(0);
    static GET_RESOURCE_READER_NAME: AtomicU64 = AtomicU64::new(0);

    let loader_name = intern_static_name(_py, &LOADER_NAME, b"loader");
    let get_resource_reader_name =
        intern_static_name(_py, &GET_RESOURCE_READER_NAME, b"get_resource_reader");

    let try_loader = |loader_bits: u64| -> Result<Option<u64>, u64> {
        let Some(call_bits) =
            importlib_reader_lookup_callable(_py, loader_bits, get_resource_reader_name)?
        else {
            return Ok(None);
        };
        let module_name_bits = alloc_str_bits(_py, module_name)?;
        let reader_bits = unsafe { call_callable1(_py, call_bits, module_name_bits) };
        dec_ref_bits(_py, module_name_bits);
        dec_ref_bits(_py, call_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        if obj_from_bits(reader_bits).is_none() {
            return Ok(None);
        }
        Ok(Some(reader_bits))
    };

    let spec_name = intern_static_name(_py, &SPEC_NAME, b"__spec__");
    if let Some(spec_bits) = getattr_optional_bits(_py, module_bits, spec_name)? {
        let mut out: Option<u64> = None;
        if let Some(loader_bits) = getattr_optional_bits(_py, spec_bits, loader_name)? {
            out = try_loader(loader_bits)?;
            if !obj_from_bits(loader_bits).is_none() {
                dec_ref_bits(_py, loader_bits);
            }
        }
        if !obj_from_bits(spec_bits).is_none() {
            dec_ref_bits(_py, spec_bits);
        }
        if out.is_some() {
            return Ok(out);
        }
    }

    if let Some(loader_bits) = getattr_optional_bits(_py, module_bits, loader_name)? {
        let out = try_loader(loader_bits)?;
        if !obj_from_bits(loader_bits).is_none() {
            dec_ref_bits(_py, loader_bits);
        }
        return Ok(out);
    }

    Ok(None)
}

fn importlib_reader_collect_unique_strings(
    _py: &PyToken<'_>,
    values_bits: u64,
    _invalid_entry_message: &str,
) -> Result<Vec<String>, u64> {
    let iter_bits = molt_iter(values_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
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
        let Some(entry) = string_obj_to_owned(obj_from_bits(pair[0])) else {
            // CPython ResourceReader handling is tolerant of non-string entries in
            // iterator-style views; skip malformed entries instead of aborting.
            continue;
        };
        if entry.is_empty() {
            continue;
        }
        if seen.insert(entry.clone()) {
            out.push(entry);
        }
    }
    Ok(out)
}

fn importlib_reader_collect_unique_paths(
    _py: &PyToken<'_>,
    values_bits: u64,
    invalid_entry_message: &str,
) -> Result<Vec<String>, u64> {
    let iter_bits = molt_iter(values_bits);
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
        let path = match path_from_bits(_py, pair[0]) {
            Ok(path_buf) => path_buf.to_string_lossy().into_owned(),
            Err(_) => {
                if exception_pending(_py) {
                    clear_exception(_py);
                }
                return Err(raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    invalid_entry_message,
                ));
            }
        };
        if path.is_empty() {
            continue;
        }
        append_unique_path(&mut out, &path);
    }
    Ok(out)
}

fn importlib_reader_collect_bytes(_py: &PyToken<'_>, value_bits: u64) -> Option<Vec<u8>> {
    let ptr = obj_from_bits(value_bits).as_ptr()?;
    unsafe { bytes_like_slice(ptr).map(|slice| slice.to_vec()) }
}

fn importlib_reader_files_traversable_bits(
    _py: &PyToken<'_>,
    reader_bits: u64,
) -> Result<Option<u64>, u64> {
    static FILES_NAME: AtomicU64 = AtomicU64::new(0);
    let files_name = intern_static_name(_py, &FILES_NAME, b"files");
    let Some(call_bits) = importlib_reader_lookup_callable(_py, reader_bits, files_name)? else {
        return Ok(None);
    };
    let value_bits = unsafe { call_callable0(_py, call_bits) };
    dec_ref_bits(_py, call_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if obj_from_bits(value_bits).is_none() {
        return Ok(None);
    }
    Ok(Some(value_bits))
}

fn importlib_traversable_joinpath_bits(
    _py: &PyToken<'_>,
    traversable_bits: u64,
    name: &str,
) -> Result<Option<u64>, u64> {
    static JOINPATH_NAME: AtomicU64 = AtomicU64::new(0);
    let joinpath_name = intern_static_name(_py, &JOINPATH_NAME, b"joinpath");
    let Some(call_bits) = importlib_reader_lookup_callable(_py, traversable_bits, joinpath_name)?
    else {
        return Ok(None);
    };
    let name_bits = alloc_str_bits(_py, name)?;
    let value_bits = unsafe { call_callable1(_py, call_bits, name_bits) };
    dec_ref_bits(_py, name_bits);
    dec_ref_bits(_py, call_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if obj_from_bits(value_bits).is_none() {
        return Ok(None);
    }
    Ok(Some(value_bits))
}

fn importlib_traversable_bits_for_parts(
    _py: &PyToken<'_>,
    reader_bits: u64,
    parts: &[String],
) -> Result<Option<u64>, u64> {
    let Some(mut current_bits) = importlib_reader_files_traversable_bits(_py, reader_bits)? else {
        return Ok(None);
    };
    for part in parts {
        if part.is_empty() {
            continue;
        }
        let next_bits = match importlib_traversable_joinpath_bits(_py, current_bits, part)? {
            Some(bits) => bits,
            None => {
                if !obj_from_bits(current_bits).is_none() {
                    dec_ref_bits(_py, current_bits);
                }
                return Ok(None);
            }
        };
        if !obj_from_bits(current_bits).is_none() {
            dec_ref_bits(_py, current_bits);
        }
        current_bits = next_bits;
    }
    Ok(Some(current_bits))
}

fn importlib_traversable_iterdir_names(
    _py: &PyToken<'_>,
    traversable_bits: u64,
) -> Result<Vec<String>, u64> {
    static ITERDIR_NAME: AtomicU64 = AtomicU64::new(0);
    static NAME_NAME: AtomicU64 = AtomicU64::new(0);
    let iterdir_name = intern_static_name(_py, &ITERDIR_NAME, b"iterdir");
    let Some(call_bits) = importlib_reader_lookup_callable(_py, traversable_bits, iterdir_name)?
    else {
        return Ok(Vec::new());
    };
    let iterable_bits = unsafe { call_callable0(_py, call_bits) };
    dec_ref_bits(_py, call_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let iter_bits = molt_iter(iterable_bits);
    if !obj_from_bits(iterable_bits).is_none() {
        dec_ref_bits(_py, iterable_bits);
    }
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let name_attr = intern_static_name(_py, &NAME_NAME, b"name");
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
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
        let entry_bits = pair[0];
        let missing = missing_bits(_py);
        let name_bits = molt_getattr_builtin(entry_bits, name_attr, missing);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        if is_missing_bits(_py, name_bits) {
            if !obj_from_bits(name_bits).is_none() {
                dec_ref_bits(_py, name_bits);
            }
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "invalid loader resource traversable payload: missing name",
            ));
        }
        let Some(name) = string_obj_to_owned(obj_from_bits(name_bits)) else {
            if !obj_from_bits(name_bits).is_none() {
                dec_ref_bits(_py, name_bits);
            }
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "invalid loader resource traversable payload: name must be str",
            ));
        };
        if !obj_from_bits(name_bits).is_none() {
            dec_ref_bits(_py, name_bits);
        }
        if name.is_empty() {
            continue;
        }
        if seen.insert(name.clone()) {
            out.push(name);
        }
    }
    Ok(out)
}

fn importlib_traversable_is_file(_py: &PyToken<'_>, traversable_bits: u64) -> Result<bool, u64> {
    static IS_FILE_NAME: AtomicU64 = AtomicU64::new(0);
    let is_file_name = intern_static_name(_py, &IS_FILE_NAME, b"is_file");
    let Some(call_bits) = importlib_reader_lookup_callable(_py, traversable_bits, is_file_name)?
    else {
        return Ok(false);
    };
    let value_bits = unsafe { call_callable0(_py, call_bits) };
    dec_ref_bits(_py, call_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let true_bits = MoltObject::from_bool(true).bits();
    let false_bits = MoltObject::from_bool(false).bits();
    if value_bits == true_bits {
        if !obj_from_bits(value_bits).is_none() {
            dec_ref_bits(_py, value_bits);
        }
        return Ok(true);
    }
    if value_bits == false_bits {
        if !obj_from_bits(value_bits).is_none() {
            dec_ref_bits(_py, value_bits);
        }
        return Ok(false);
    }
    if !obj_from_bits(value_bits).is_none() {
        dec_ref_bits(_py, value_bits);
    }
    Err(raise_exception::<_>(
        _py,
        "RuntimeError",
        "invalid loader resource traversable payload: is_file must be bool",
    ))
}

fn importlib_traversable_is_dir(_py: &PyToken<'_>, traversable_bits: u64) -> Result<bool, u64> {
    static IS_DIR_NAME: AtomicU64 = AtomicU64::new(0);
    let is_dir_name = intern_static_name(_py, &IS_DIR_NAME, b"is_dir");
    if let Some(call_bits) = importlib_reader_lookup_callable(_py, traversable_bits, is_dir_name)? {
        let value_bits = unsafe { call_callable0(_py, call_bits) };
        dec_ref_bits(_py, call_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        let true_bits = MoltObject::from_bool(true).bits();
        let false_bits = MoltObject::from_bool(false).bits();
        if value_bits == true_bits {
            if !obj_from_bits(value_bits).is_none() {
                dec_ref_bits(_py, value_bits);
            }
            return Ok(true);
        }
        if value_bits == false_bits {
            if !obj_from_bits(value_bits).is_none() {
                dec_ref_bits(_py, value_bits);
            }
            return Ok(false);
        }
        if !obj_from_bits(value_bits).is_none() {
            dec_ref_bits(_py, value_bits);
        }
        return Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            "invalid loader resource traversable payload: is_dir must be bool",
        ));
    }

    match path_from_bits(_py, traversable_bits) {
        Ok(path) => Ok(std::fs::metadata(path)
            .map(|meta| meta.is_dir())
            .unwrap_or(false)),
        Err(_) => {
            if exception_pending(_py) {
                clear_exception(_py);
            }
            Ok(false)
        }
    }
}

fn importlib_traversable_exists(_py: &PyToken<'_>, traversable_bits: u64) -> Result<bool, u64> {
    static EXISTS_NAME: AtomicU64 = AtomicU64::new(0);
    let exists_name = intern_static_name(_py, &EXISTS_NAME, b"exists");
    if let Some(call_bits) = importlib_reader_lookup_callable(_py, traversable_bits, exists_name)? {
        let value_bits = unsafe { call_callable0(_py, call_bits) };
        dec_ref_bits(_py, call_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        let true_bits = MoltObject::from_bool(true).bits();
        let false_bits = MoltObject::from_bool(false).bits();
        if value_bits == true_bits {
            if !obj_from_bits(value_bits).is_none() {
                dec_ref_bits(_py, value_bits);
            }
            return Ok(true);
        }
        if value_bits == false_bits {
            if !obj_from_bits(value_bits).is_none() {
                dec_ref_bits(_py, value_bits);
            }
            return Ok(false);
        }
        if !obj_from_bits(value_bits).is_none() {
            dec_ref_bits(_py, value_bits);
        }
        return Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            "invalid loader resource traversable payload: exists must be bool",
        ));
    }

    match path_from_bits(_py, traversable_bits) {
        Ok(path) => Ok(std::fs::metadata(path).is_ok()),
        Err(_) => {
            if exception_pending(_py) {
                clear_exception(_py);
            }
            let is_file = importlib_traversable_is_file(_py, traversable_bits)?;
            if is_file {
                return Ok(true);
            }
            importlib_traversable_is_dir(_py, traversable_bits)
        }
    }
}

fn importlib_traversable_open_bytes(
    _py: &PyToken<'_>,
    traversable_bits: u64,
) -> Result<Vec<u8>, u64> {
    static OPEN_NAME: AtomicU64 = AtomicU64::new(0);
    static READ_NAME: AtomicU64 = AtomicU64::new(0);
    static CLOSE_NAME: AtomicU64 = AtomicU64::new(0);
    let open_name = intern_static_name(_py, &OPEN_NAME, b"open");
    let Some(call_bits) = importlib_reader_lookup_callable(_py, traversable_bits, open_name)?
    else {
        return Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            "invalid loader resource traversable payload: missing open()",
        ));
    };
    let mode_bits = alloc_str_bits(_py, "rb")?;
    let handle_bits = unsafe { call_callable1(_py, call_bits, mode_bits) };
    dec_ref_bits(_py, mode_bits);
    dec_ref_bits(_py, call_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if let Some(bytes) = importlib_reader_collect_bytes(_py, handle_bits) {
        if !obj_from_bits(handle_bits).is_none() {
            dec_ref_bits(_py, handle_bits);
        }
        return Ok(bytes);
    }
    let read_name = intern_static_name(_py, &READ_NAME, b"read");
    let read_bits = match importlib_reader_lookup_callable(_py, handle_bits, read_name)? {
        Some(bits) => bits,
        None => {
            if !obj_from_bits(handle_bits).is_none() {
                dec_ref_bits(_py, handle_bits);
            }
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "invalid loader open_resource payload",
            ));
        }
    };
    let payload_bits = unsafe { call_callable0(_py, read_bits) };
    dec_ref_bits(_py, read_bits);
    if exception_pending(_py) {
        if !obj_from_bits(handle_bits).is_none() {
            dec_ref_bits(_py, handle_bits);
        }
        return Err(MoltObject::none().bits());
    }
    let close_name = intern_static_name(_py, &CLOSE_NAME, b"close");
    if let Some(close_bits) = importlib_reader_lookup_callable(_py, handle_bits, close_name)? {
        let _ = unsafe { call_callable0(_py, close_bits) };
        dec_ref_bits(_py, close_bits);
        if exception_pending(_py) {
            if !obj_from_bits(payload_bits).is_none() {
                dec_ref_bits(_py, payload_bits);
            }
            if !obj_from_bits(handle_bits).is_none() {
                dec_ref_bits(_py, handle_bits);
            }
            return Err(MoltObject::none().bits());
        }
    }
    if !obj_from_bits(handle_bits).is_none() {
        dec_ref_bits(_py, handle_bits);
    }
    let Some(bytes) = importlib_reader_collect_bytes(_py, payload_bits) else {
        if !obj_from_bits(payload_bits).is_none() {
            dec_ref_bits(_py, payload_bits);
        }
        return Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            "invalid loader open_resource payload",
        ));
    };
    if !obj_from_bits(payload_bits).is_none() {
        dec_ref_bits(_py, payload_bits);
    }
    Ok(bytes)
}

fn importlib_reader_files_root_path(
    _py: &PyToken<'_>,
    reader_bits: u64,
) -> Result<Option<String>, u64> {
    let Some(value_bits) = importlib_reader_files_traversable_bits(_py, reader_bits)? else {
        return Ok(None);
    };
    let out = match path_from_bits(_py, value_bits) {
        Ok(path) => {
            let text = path.to_string_lossy().into_owned();
            if text.is_empty() { None } else { Some(text) }
        }
        Err(_) => {
            if exception_pending(_py) {
                clear_exception(_py);
            }
            None
        }
    };
    if !obj_from_bits(value_bits).is_none() {
        dec_ref_bits(_py, value_bits);
    }
    Ok(out)
}

fn importlib_reader_join_parts_path(root: &str, parts: &[String]) -> String {
    let sep = bootstrap_path_sep();
    let mut path = root.to_string();
    for part in parts {
        if part.is_empty() {
            continue;
        }
        path = path_join_text(path, part, sep);
    }
    path
}

fn importlib_reader_root_payload_for_parts(
    _py: &PyToken<'_>,
    reader_bits: u64,
    parts: &[String],
) -> Result<Option<(String, ImportlibResourcesPathPayload)>, u64> {
    let Some(root) = importlib_reader_files_root_path(_py, reader_bits)? else {
        return Ok(None);
    };
    let joined = importlib_reader_join_parts_path(&root, parts);
    let payload = importlib_resources_path_payload(&joined);
    Ok(Some((joined, payload)))
}

fn importlib_resources_reader_roots_impl(
    _py: &PyToken<'_>,
    reader_bits: u64,
) -> Result<Vec<String>, u64> {
    static MOLT_ROOTS_NAME: AtomicU64 = AtomicU64::new(0);
    let molt_roots_name = intern_static_name(_py, &MOLT_ROOTS_NAME, b"molt_roots");
    if let Some(call_bits) = importlib_reader_lookup_callable(_py, reader_bits, molt_roots_name)? {
        let values_bits = unsafe { call_callable0(_py, call_bits) };
        dec_ref_bits(_py, call_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        let roots = importlib_reader_collect_unique_paths(
            _py,
            values_bits,
            "invalid loader resource roots payload: list expected",
        )?;
        if !obj_from_bits(values_bits).is_none() {
            dec_ref_bits(_py, values_bits);
        }
        if !roots.is_empty() {
            return Ok(roots);
        }
    }

    if let Some(root) = importlib_reader_files_root_path(_py, reader_bits)? {
        return Ok(vec![root]);
    }

    Ok(Vec::new())
}

fn importlib_resources_reader_contents_impl(
    _py: &PyToken<'_>,
    reader_bits: u64,
) -> Result<Vec<String>, u64> {
    static CONTENTS_NAME: AtomicU64 = AtomicU64::new(0);
    let contents_name = intern_static_name(_py, &CONTENTS_NAME, b"contents");
    if let Some(call_bits) = importlib_reader_lookup_callable(_py, reader_bits, contents_name)? {
        let values_bits = unsafe { call_callable0(_py, call_bits) };
        dec_ref_bits(_py, call_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        if !obj_from_bits(values_bits).is_none() {
            let out = importlib_reader_collect_unique_strings(
                _py,
                values_bits,
                "invalid loader resource reader contents payload",
            )?;
            if !obj_from_bits(values_bits).is_none() {
                dec_ref_bits(_py, values_bits);
            }
            if !out.is_empty() {
                return Ok(out);
            }
        }
    }
    if let Some(root_bits) = importlib_traversable_bits_for_parts(_py, reader_bits, &[])? {
        let out = importlib_traversable_iterdir_names(_py, root_bits);
        if !obj_from_bits(root_bits).is_none() {
            dec_ref_bits(_py, root_bits);
        }
        return out;
    }
    if let Some((_root, payload)) = importlib_reader_root_payload_for_parts(_py, reader_bits, &[])?
        && payload.is_dir
    {
        return Ok(payload.entries);
    }
    Ok(Vec::new())
}

fn importlib_resources_reader_resource_path_impl(
    _py: &PyToken<'_>,
    reader_bits: u64,
    name: &str,
) -> Result<Option<String>, u64> {
    static RESOURCE_PATH_NAME: AtomicU64 = AtomicU64::new(0);
    let resource_path_name = intern_static_name(_py, &RESOURCE_PATH_NAME, b"resource_path");
    if let Some(call_bits) = importlib_reader_lookup_callable(_py, reader_bits, resource_path_name)?
    {
        let name_bits = alloc_str_bits(_py, name)?;
        let value_bits = unsafe { call_callable1(_py, call_bits, name_bits) };
        dec_ref_bits(_py, name_bits);
        dec_ref_bits(_py, call_bits);
        if exception_pending(_py) {
            if clear_pending_if_kind(
                _py,
                &["FileNotFoundError", "OSError", "NotImplementedError"],
            ) {
                return Ok(None);
            }
            return Err(MoltObject::none().bits());
        }
        let path = match path_from_bits(_py, value_bits) {
            Ok(path_buf) => path_buf.to_string_lossy().into_owned(),
            Err(_) => {
                if exception_pending(_py) {
                    clear_exception(_py);
                }
                if !obj_from_bits(value_bits).is_none() {
                    dec_ref_bits(_py, value_bits);
                }
                return Err(raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "invalid loader resource path payload",
                ));
            }
        };
        if !obj_from_bits(value_bits).is_none() {
            dec_ref_bits(_py, value_bits);
        }
        let payload = importlib_resources_path_payload(&path);
        if payload.is_file && !payload.is_archive_member {
            return Ok(Some(path));
        }
        return Ok(None);
    }

    let parts = importlib_resources_name_parts(name);
    if let Some(target_bits) = importlib_traversable_bits_for_parts(_py, reader_bits, &parts)? {
        let out = match path_from_bits(_py, target_bits) {
            Ok(path_buf) => {
                let path = path_buf.to_string_lossy().into_owned();
                let payload = importlib_resources_path_payload(&path);
                if payload.is_file && !payload.is_archive_member {
                    Some(path)
                } else {
                    None
                }
            }
            Err(_) => {
                if exception_pending(_py) {
                    clear_exception(_py);
                }
                None
            }
        };
        if !obj_from_bits(target_bits).is_none() {
            dec_ref_bits(_py, target_bits);
        }
        return Ok(out);
    }
    if let Some((joined, payload)) =
        importlib_reader_root_payload_for_parts(_py, reader_bits, &parts)?
        && payload.is_file
        && !payload.is_archive_member
    {
        return Ok(Some(joined));
    }
    Ok(None)
}

fn importlib_resources_reader_child_names_impl(
    _py: &PyToken<'_>,
    reader_bits: u64,
    parts: &[String],
) -> Result<Vec<String>, u64> {
    if let Some(target_bits) = importlib_traversable_bits_for_parts(_py, reader_bits, parts)? {
        let names = importlib_traversable_iterdir_names(_py, target_bits);
        if !obj_from_bits(target_bits).is_none() {
            dec_ref_bits(_py, target_bits);
        }
        if let Ok(values) = names {
            return Ok(values);
        }
        return names;
    }
    if let Some((_joined, payload)) =
        importlib_reader_root_payload_for_parts(_py, reader_bits, parts)?
        && payload.is_dir
    {
        return Ok(payload.entries);
    }
    let entries = importlib_resources_reader_contents_impl(_py, reader_bits)?;
    let prefix = parts.join("/");
    let mut out: Vec<String> = Vec::new();
    let mut seen: HashSet<String> = HashSet::new();
    for entry in entries {
        let remainder = if prefix.is_empty() {
            entry.as_str()
        } else if let Some(value) = entry.strip_prefix(&format!("{prefix}/")) {
            value
        } else {
            continue;
        };
        if remainder.is_empty() {
            continue;
        }
        let Some(child) = remainder.split('/').next() else {
            continue;
        };
        if child.is_empty() {
            continue;
        }
        let child_name = child.to_string();
        if seen.insert(child_name.clone()) {
            out.push(child_name);
        }
    }
    Ok(out)
}

fn importlib_resources_reader_exists_impl(
    _py: &PyToken<'_>,
    reader_bits: u64,
    parts: &[String],
) -> Result<bool, u64> {
    if parts.is_empty() {
        return Ok(true);
    }
    if let Some(target_bits) = importlib_traversable_bits_for_parts(_py, reader_bits, parts)? {
        let out = importlib_traversable_exists(_py, target_bits);
        if !obj_from_bits(target_bits).is_none() {
            dec_ref_bits(_py, target_bits);
        }
        return out;
    }
    if let Some((_joined, payload)) =
        importlib_reader_root_payload_for_parts(_py, reader_bits, parts)?
    {
        return Ok(payload.exists);
    }
    let name = parts.join("/");
    if !name.is_empty() && importlib_resources_reader_is_resource_impl(_py, reader_bits, &name)? {
        return Ok(true);
    }
    let children = importlib_resources_reader_child_names_impl(_py, reader_bits, parts)?;
    Ok(!children.is_empty())
}

fn importlib_resources_reader_is_dir_impl(
    _py: &PyToken<'_>,
    reader_bits: u64,
    parts: &[String],
) -> Result<bool, u64> {
    if parts.is_empty() {
        return Ok(true);
    }
    if let Some(target_bits) = importlib_traversable_bits_for_parts(_py, reader_bits, parts)? {
        let is_dir = importlib_traversable_is_dir(_py, target_bits)?;
        if is_dir {
            if !obj_from_bits(target_bits).is_none() {
                dec_ref_bits(_py, target_bits);
            }
            return Ok(true);
        }
        let is_file = importlib_traversable_is_file(_py, target_bits)?;
        if !obj_from_bits(target_bits).is_none() {
            dec_ref_bits(_py, target_bits);
        }
        return Ok(!is_file);
    }
    if let Some((_joined, payload)) =
        importlib_reader_root_payload_for_parts(_py, reader_bits, parts)?
    {
        return Ok(payload.is_dir);
    }
    let name = parts.join("/");
    if !name.is_empty() && importlib_resources_reader_is_resource_impl(_py, reader_bits, &name)? {
        return Ok(false);
    }
    let children = importlib_resources_reader_child_names_impl(_py, reader_bits, parts)?;
    Ok(!children.is_empty())
}

fn importlib_resources_reader_is_resource_impl(
    _py: &PyToken<'_>,
    reader_bits: u64,
    name: &str,
) -> Result<bool, u64> {
    static IS_RESOURCE_NAME: AtomicU64 = AtomicU64::new(0);
    let is_resource_name = intern_static_name(_py, &IS_RESOURCE_NAME, b"is_resource");
    if let Some(call_bits) = importlib_reader_lookup_callable(_py, reader_bits, is_resource_name)? {
        let name_bits = alloc_str_bits(_py, name)?;
        let value_bits = unsafe { call_callable1(_py, call_bits, name_bits) };
        dec_ref_bits(_py, name_bits);
        dec_ref_bits(_py, call_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        let true_bits = MoltObject::from_bool(true).bits();
        let false_bits = MoltObject::from_bool(false).bits();
        if value_bits == true_bits {
            if !obj_from_bits(value_bits).is_none() {
                dec_ref_bits(_py, value_bits);
            }
            return Ok(true);
        }
        if value_bits == false_bits {
            if !obj_from_bits(value_bits).is_none() {
                dec_ref_bits(_py, value_bits);
            }
            return Ok(false);
        }
        if !obj_from_bits(value_bits).is_none() {
            dec_ref_bits(_py, value_bits);
        }
        return Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            "invalid loader resource is_resource payload",
        ));
    }
    let parts = importlib_resources_name_parts(name);
    let Some(target_bits) = importlib_traversable_bits_for_parts(_py, reader_bits, &parts)? else {
        if let Some((_joined, payload)) =
            importlib_reader_root_payload_for_parts(_py, reader_bits, &parts)?
        {
            return Ok(payload.is_file);
        }
        return Ok(false);
    };
    let out = importlib_traversable_is_file(_py, target_bits);
    if !obj_from_bits(target_bits).is_none() {
        dec_ref_bits(_py, target_bits);
    }
    out
}

fn importlib_resources_reader_open_resource_bytes_impl(
    _py: &PyToken<'_>,
    reader_bits: u64,
    name: &str,
) -> Result<Vec<u8>, u64> {
    static OPEN_RESOURCE_NAME: AtomicU64 = AtomicU64::new(0);
    static READ_NAME: AtomicU64 = AtomicU64::new(0);
    static CLOSE_NAME: AtomicU64 = AtomicU64::new(0);

    let open_resource_name = intern_static_name(_py, &OPEN_RESOURCE_NAME, b"open_resource");
    if let Some(call_bits) = importlib_reader_lookup_callable(_py, reader_bits, open_resource_name)?
    {
        let name_bits = alloc_str_bits(_py, name)?;
        let handle_bits = unsafe { call_callable1(_py, call_bits, name_bits) };
        dec_ref_bits(_py, name_bits);
        dec_ref_bits(_py, call_bits);
        if exception_pending(_py) {
            if clear_pending_if_kind(_py, &["NotImplementedError"]) {
                let parts = importlib_resources_name_parts(name);
                let Some(target_bits) =
                    importlib_traversable_bits_for_parts(_py, reader_bits, &parts)?
                else {
                    return Err(raise_exception::<_>(_py, "FileNotFoundError", name));
                };
                let out = importlib_traversable_open_bytes(_py, target_bits);
                if !obj_from_bits(target_bits).is_none() {
                    dec_ref_bits(_py, target_bits);
                }
                return out;
            }
            return Err(MoltObject::none().bits());
        }

        if let Some(bytes) = importlib_reader_collect_bytes(_py, handle_bits) {
            if !obj_from_bits(handle_bits).is_none() {
                dec_ref_bits(_py, handle_bits);
            }
            return Ok(bytes);
        }

        let read_name = intern_static_name(_py, &READ_NAME, b"read");
        let read_bits = match importlib_reader_lookup_callable(_py, handle_bits, read_name)? {
            Some(bits) => bits,
            None => {
                if !obj_from_bits(handle_bits).is_none() {
                    dec_ref_bits(_py, handle_bits);
                }
                return Err(raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "invalid loader open_resource payload",
                ));
            }
        };
        let payload_bits = unsafe { call_callable0(_py, read_bits) };
        dec_ref_bits(_py, read_bits);
        if exception_pending(_py) {
            if !obj_from_bits(handle_bits).is_none() {
                dec_ref_bits(_py, handle_bits);
            }
            return Err(MoltObject::none().bits());
        }

        let close_name = intern_static_name(_py, &CLOSE_NAME, b"close");
        if let Some(close_bits) = importlib_reader_lookup_callable(_py, handle_bits, close_name)? {
            let _ = unsafe { call_callable0(_py, close_bits) };
            dec_ref_bits(_py, close_bits);
            if exception_pending(_py) {
                if !obj_from_bits(payload_bits).is_none() {
                    dec_ref_bits(_py, payload_bits);
                }
                if !obj_from_bits(handle_bits).is_none() {
                    dec_ref_bits(_py, handle_bits);
                }
                return Err(MoltObject::none().bits());
            }
        }
        if !obj_from_bits(handle_bits).is_none() {
            dec_ref_bits(_py, handle_bits);
        }

        let Some(bytes) = importlib_reader_collect_bytes(_py, payload_bits) else {
            if exception_pending(_py) {
                clear_exception(_py);
            }
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "invalid loader open_resource payload",
            ));
        };
        if !obj_from_bits(payload_bits).is_none() {
            dec_ref_bits(_py, payload_bits);
        }
        return Ok(bytes);
    }

    let parts = importlib_resources_name_parts(name);
    if let Some(target_bits) = importlib_traversable_bits_for_parts(_py, reader_bits, &parts)? {
        let out = importlib_traversable_open_bytes(_py, target_bits);
        if !obj_from_bits(target_bits).is_none() {
            dec_ref_bits(_py, target_bits);
        }
        return out;
    }
    if let Some((joined, payload)) =
        importlib_reader_root_payload_for_parts(_py, reader_bits, &parts)?
    {
        if !payload.is_file {
            return Err(raise_exception::<_>(_py, "FileNotFoundError", name));
        }
        return importlib_read_file_bytes(_py, &joined);
    }
    Err(raise_exception::<_>(_py, "FileNotFoundError", name))
}

fn importlib_extension_shim_candidates(module_name: &str, path: &str) -> Vec<String> {
    let sep = bootstrap_path_sep();
    let mut out: Vec<String> = Vec::new();
    append_unique_path(&mut out, &format!("{path}.molt.py"));
    append_unique_path(&mut out, &format!("{path}.py"));
    if let Some(stripped) = path.rsplit_once('.').map(|(prefix, _)| prefix) {
        append_unique_path(&mut out, &format!("{stripped}.molt.py"));
        append_unique_path(&mut out, &format!("{stripped}.py"));
        if let Some((prefix, _)) = stripped.rsplit_once(".cpython-") {
            append_unique_path(&mut out, &format!("{prefix}.molt.py"));
            append_unique_path(&mut out, &format!("{prefix}.py"));
        }
        if let Some((prefix, _)) = stripped.rsplit_once(".abi") {
            append_unique_path(&mut out, &format!("{prefix}.molt.py"));
            append_unique_path(&mut out, &format!("{prefix}.py"));
        }
        if let Some((prefix, _)) = stripped.rsplit_once(".cp") {
            append_unique_path(&mut out, &format!("{prefix}.molt.py"));
            append_unique_path(&mut out, &format!("{prefix}.py"));
        }
    }
    let dirname = path_dirname_text(path, sep);
    if path_basename_text(&dirname, sep) == "__pycache__" {
        let parent = path_dirname_text(&dirname, sep);
        let basename = path_basename_text(path, sep);
        let stem = basename
            .rsplit_once('.')
            .map(|(value, _)| value)
            .unwrap_or(&basename);
        let module_stem = stem.split('.').next().unwrap_or(stem);
        if !module_stem.is_empty() {
            let molt_candidate =
                path_join_text(parent.clone(), &format!("{module_stem}.molt.py"), sep);
            append_unique_path(&mut out, &molt_candidate);
            let py_candidate = path_join_text(parent, &format!("{module_stem}.py"), sep);
            append_unique_path(&mut out, &py_candidate);
        }
    }
    let dirname = path_dirname_text(path, sep);
    let local_name = module_name.rsplit('.').next().unwrap_or(module_name);
    if !local_name.is_empty() {
        let named_molt = path_join_text(dirname.clone(), &format!("{local_name}.molt.py"), sep);
        append_unique_path(&mut out, &named_molt);
        let named_py = path_join_text(dirname, &format!("{local_name}.py"), sep);
        append_unique_path(&mut out, &named_py);
        let package_dir = path_join_text(path_dirname_text(path, sep), local_name, sep);
        let pkg_init_molt = path_join_text(package_dir.clone(), "__init__.molt.py", sep);
        append_unique_path(&mut out, &pkg_init_molt);
        let pkg_init_py = path_join_text(package_dir, "__init__.py", sep);
        append_unique_path(&mut out, &pkg_init_py);
    }
    let basename = path_basename_text(path, sep);
    if basename.starts_with("__init__.") {
        append_unique_path(
            &mut out,
            &path_join_text(path_dirname_text(path, sep), "__init__.molt.py", sep),
        );
        append_unique_path(
            &mut out,
            &path_join_text(path_dirname_text(path, sep), "__init__.py", sep),
        );
    }
    out
}

fn importlib_sourceless_source_candidates(module_name: &str, path: &str) -> Vec<String> {
    let sep = bootstrap_path_sep();
    let mut out: Vec<String> = Vec::new();
    if let Some(stripped) = path.strip_suffix(".pyc") {
        append_unique_path(&mut out, &format!("{stripped}.molt.py"));
        append_unique_path(&mut out, &format!("{stripped}.py"));
        if let Some((prefix, _)) = stripped.rsplit_once(".cpython-") {
            append_unique_path(&mut out, &format!("{prefix}.molt.py"));
            append_unique_path(&mut out, &format!("{prefix}.py"));
        }
        if let Some((prefix, _)) = stripped.rsplit_once(".pypy-") {
            append_unique_path(&mut out, &format!("{prefix}.molt.py"));
            append_unique_path(&mut out, &format!("{prefix}.py"));
        }
    }
    let dirname = path_dirname_text(path, sep);
    if path_basename_text(&dirname, sep) == "__pycache__" {
        let parent = path_dirname_text(&dirname, sep);
        let basename = path_basename_text(path, sep);
        let stem = basename.trim_end_matches(".pyc");
        let module_name = stem.split('.').next().unwrap_or(stem);
        if !module_name.is_empty() {
            let molt_candidate =
                path_join_text(parent.clone(), &format!("{module_name}.molt.py"), sep);
            append_unique_path(&mut out, &molt_candidate);
            let candidate = path_join_text(parent, &format!("{module_name}.py"), sep);
            append_unique_path(&mut out, &candidate);
        }
    }
    let dirname = path_dirname_text(path, sep);
    let local_name = module_name.rsplit('.').next().unwrap_or(module_name);
    if !local_name.is_empty() {
        let named_molt = path_join_text(dirname.clone(), &format!("{local_name}.molt.py"), sep);
        append_unique_path(&mut out, &named_molt);
        let named_py = path_join_text(dirname, &format!("{local_name}.py"), sep);
        append_unique_path(&mut out, &named_py);
    }
    let basename = path_basename_text(path, sep);
    if basename.starts_with("__init__.") && basename.ends_with(".pyc") {
        append_unique_path(
            &mut out,
            &path_join_text(path_dirname_text(path, sep), "__init__.molt.py", sep),
        );
        append_unique_path(
            &mut out,
            &path_join_text(path_dirname_text(path, sep), "__init__.py", sep),
        );
    }
    out
}

fn bootstrap_resolve_abspath(path: &str, module_file: Option<String>) -> String {
    let sep = bootstrap_path_sep();
    let state = sys_bootstrap_state_from_module_file(module_file);
    let joined = if path_is_absolute_text(path, sep) || state.pwd.is_empty() {
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

fn importlib_loader_resolution_payload_bits(
    _py: &PyToken<'_>,
    resolution: &SourceLoaderResolution,
) -> Result<u64, u64> {
    let module_package_bits = alloc_str_bits(_py, &resolution.module_package)?;
    let package_root_bits = match resolution.package_root.as_deref() {
        Some(root) => match alloc_str_bits(_py, root) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, module_package_bits);
                return Err(err);
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
            return Err(MoltObject::none().bits());
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
        Err(MoltObject::none().bits())
    } else {
        Ok(MoltObject::from_ptr(dict_ptr).bits())
    }
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

fn bytes_arg_from_bits(_py: &PyToken<'_>, bits: u64, name: &str) -> Result<Vec<u8>, u64> {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{name} must be bytes-like"),
        ));
    };
    let Some(slice) = (unsafe { bytes_like_slice(ptr) }) else {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{name} must be bytes-like"),
        ));
    };
    Ok(slice.to_vec())
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

fn importlib_target_minor(_py: &PyToken<'_>) -> i64 {
    let state = crate::runtime_state(_py);
    if let Some(info) = state.sys_version_info.lock().unwrap().as_ref()
        && info.major == 3
    {
        return info.minor;
    }
    if let Ok(raw) = std::env::var("MOLT_PYTHON_VERSION")
        && let Some((major_raw, minor_raw)) = raw.split_once('.')
        && major_raw.trim() == "3"
        && let Ok(minor) = minor_raw.trim().parse::<i64>()
    {
        return minor;
    }
    if let Ok(raw) = std::env::var("MOLT_SYS_VERSION_INFO") {
        let mut parts = raw.split(',');
        if let (Some(major_raw), Some(minor_raw)) = (parts.next(), parts.next())
            && major_raw.trim() == "3"
            && let Ok(minor) = minor_raw.trim().parse::<i64>()
        {
            return minor;
        }
    }
    12
}

const REMOVED_STDLIB_MODULES_313: [&str; 20] = [
    "aifc",
    "audioop",
    "cgi",
    "cgitb",
    "chunk",
    "crypt",
    "imghdr",
    "mailcap",
    "msilib",
    "nis",
    "nntplib",
    "ossaudiodev",
    "pipes",
    "sndhdr",
    "spwd",
    "sunau",
    "telnetlib",
    "tkinter.tix",
    "uu",
    "xdrlib",
];

fn removed_stdlib_313_missing_name(resolved: &str) -> Option<&'static str> {
    REMOVED_STDLIB_MODULES_313.iter().copied().find(|&module| {
        resolved == module
            || resolved
                .strip_prefix(module)
                .is_some_and(|tail| tail.starts_with('.'))
    })
}

fn importlib_known_absent_missing_name(_py: &PyToken<'_>, resolved: &str) -> Option<String> {
    let target_minor = importlib_target_minor(_py);
    if target_minor >= 13
        && let Some(missing_name) = removed_stdlib_313_missing_name(resolved)
    {
        return Some(missing_name.to_string());
    }
    match resolved {
        "asyncio.graph" if target_minor < 14 => Some(resolved.to_string()),
        "json.__main__" if target_minor < 14 => Some(resolved.to_string()),
        "_android_support" if !cfg!(target_os = "android") => Some(resolved.to_string()),
        "_remote_debugging" if target_minor < 13 => Some(resolved.to_string()),
        "_interpchannels" if target_minor < 13 => Some(resolved.to_string()),
        "_opcode_metadata" if target_minor < 14 => Some(resolved.to_string()),
        "importlib.metadata.diagnose" if target_minor < 13 => Some(resolved.to_string()),
        "importlib.resources._functional" => Some(resolved.to_string()),
        "encodings._win_cp_codecs" if !cfg!(target_os = "windows") => Some(resolved.to_string()),
        "multiprocessing.popen_spawn_win32" if !cfg!(target_os = "windows") => {
            Some(String::from("msvcrt"))
        }
        _ => None,
    }
}

const IMPORTLIB_SPEC_FIRST_IMPORTS: [&str; 1] = ["asyncio.graph"];
const IMPORTLIB_EMPTY_MODULE_RETRY_PREFIXES: [&str; 1] = ["multiprocessing"];

fn importlib_modules_runtime_error(_py: &PyToken<'_>) -> u64 {
    raise_exception::<_>(
        _py,
        "RuntimeError",
        "invalid importlib runtime state payload: modules",
    )
}

fn importlib_runtime_modules_bits(_py: &PyToken<'_>) -> Result<u64, u64> {
    static MODULES_NAME: AtomicU64 = AtomicU64::new(0);
    let sys_bits = {
        let cache = crate::builtins::exceptions::internals::module_cache(_py);
        let guard = cache.lock().unwrap();
        guard.get("sys").copied()
    };
    let Some(sys_bits) = sys_bits else {
        return Err(importlib_modules_runtime_error(_py));
    };
    if obj_from_bits(sys_bits).is_none() {
        return Err(importlib_modules_runtime_error(_py));
    }
    let modules_bits = importlib_runtime_state_attr_bits(_py, sys_bits, &MODULES_NAME, b"modules")?;
    let Some(modules_ptr) = obj_from_bits(modules_bits).as_ptr() else {
        if !obj_from_bits(modules_bits).is_none() {
            dec_ref_bits(_py, modules_bits);
        }
        return Err(importlib_modules_runtime_error(_py));
    };
    if unsafe { object_type_id(modules_ptr) } != TYPE_ID_DICT {
        if !obj_from_bits(modules_bits).is_none() {
            dec_ref_bits(_py, modules_bits);
        }
        return Err(importlib_modules_runtime_error(_py));
    }
    Ok(modules_bits)
}

fn importlib_dict_get_string_key_bits(
    _py: &PyToken<'_>,
    dict_ptr: *mut u8,
    key_bits: u64,
) -> Result<Option<u64>, u64> {
    let value_bits = unsafe { dict_get_in_place(_py, dict_ptr, key_bits) };
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(value_bits.filter(|bits| !obj_from_bits(*bits).is_none()))
}

fn importlib_dict_del_string_key(_py: &PyToken<'_>, dict_ptr: *mut u8, key_bits: u64) {
    unsafe {
        let _ = dict_del_in_place(_py, dict_ptr, key_bits);
    }
    if exception_pending(_py) {
        clear_exception(_py);
    }
}

fn importlib_dict_set_string_key(
    _py: &PyToken<'_>,
    dict_ptr: *mut u8,
    key_bits: u64,
    value_bits: u64,
) -> Result<(), u64> {
    unsafe {
        dict_set_in_place(_py, dict_ptr, key_bits, value_bits);
    }
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(())
}

fn importlib_existing_spec_from_modules_bits(
    _py: &PyToken<'_>,
    module_name: &str,
    modules_bits: u64,
    machinery_bits: u64,
) -> Result<u64, u64> {
    static SPEC_NAME: AtomicU64 = AtomicU64::new(0);
    static FILE_NAME: AtomicU64 = AtomicU64::new(0);
    static MODULE_SPEC_NAME: AtomicU64 = AtomicU64::new(0);

    let Some(modules_ptr) = obj_from_bits(modules_bits).as_ptr() else {
        return Err(importlib_modules_runtime_error(_py));
    };
    if unsafe { object_type_id(modules_ptr) } != TYPE_ID_DICT {
        return Err(importlib_modules_runtime_error(_py));
    }

    let module_name_key_bits = alloc_str_bits(_py, module_name)?;
    let existing_bits =
        match importlib_dict_get_string_key_bits(_py, modules_ptr, module_name_key_bits) {
            Ok(value) => value,
            Err(err) => {
                if !obj_from_bits(module_name_key_bits).is_none() {
                    dec_ref_bits(_py, module_name_key_bits);
                }
                return Err(err);
            }
        };
    let Some(existing_bits) = existing_bits else {
        if !obj_from_bits(module_name_key_bits).is_none() {
            dec_ref_bits(_py, module_name_key_bits);
        }
        return Ok(MoltObject::none().bits());
    };

    let spec_name = intern_static_name(_py, &SPEC_NAME, b"__spec__");
    if let Some(spec_bits) = getattr_optional_bits(_py, existing_bits, spec_name)?
        && !obj_from_bits(spec_bits).is_none()
    {
        if !obj_from_bits(module_name_key_bits).is_none() {
            dec_ref_bits(_py, module_name_key_bits);
        }
        return Ok(spec_bits);
    }

    let file_name = intern_static_name(_py, &FILE_NAME, b"__file__");
    let origin_bits = match getattr_optional_bits(_py, existing_bits, file_name)? {
        Some(bits) => {
            if string_obj_to_owned(obj_from_bits(bits)).is_some() {
                bits
            } else {
                if !obj_from_bits(bits).is_none() {
                    dec_ref_bits(_py, bits);
                }
                MoltObject::none().bits()
            }
        }
        None => MoltObject::none().bits(),
    };
    let module_spec_cls_bits = match importlib_required_attribute(
        _py,
        machinery_bits,
        &MODULE_SPEC_NAME,
        b"ModuleSpec",
        "importlib.machinery",
    ) {
        Ok(bits) => bits,
        Err(err) => {
            if !obj_from_bits(origin_bits).is_none() {
                dec_ref_bits(_py, origin_bits);
            }
            if !obj_from_bits(module_name_key_bits).is_none() {
                dec_ref_bits(_py, module_name_key_bits);
            }
            return Err(err);
        }
    };
    let out = call_callable_positional(
        _py,
        module_spec_cls_bits,
        &[
            module_name_key_bits,
            MoltObject::none().bits(),
            origin_bits,
            MoltObject::from_bool(false).bits(),
        ],
    );
    if !obj_from_bits(module_spec_cls_bits).is_none() {
        dec_ref_bits(_py, module_spec_cls_bits);
    }
    if !obj_from_bits(origin_bits).is_none() {
        dec_ref_bits(_py, origin_bits);
    }
    if !obj_from_bits(module_name_key_bits).is_none() {
        dec_ref_bits(_py, module_name_key_bits);
    }
    out
}

fn importlib_parent_search_paths_payload(
    _py: &PyToken<'_>,
    module_name: &str,
    modules_bits: u64,
) -> Result<ImportlibParentSearchPathsPayload, u64> {
    static DUNDER_PATH_NAME: AtomicU64 = AtomicU64::new(0);

    let Some(modules_ptr) = obj_from_bits(modules_bits).as_ptr() else {
        return Err(importlib_modules_runtime_error(_py));
    };
    if unsafe { object_type_id(modules_ptr) } != TYPE_ID_DICT {
        return Err(importlib_modules_runtime_error(_py));
    }

    let parent_name = module_name
        .rsplit_once('.')
        .and_then(|(parent, _)| (!parent.is_empty()).then_some(parent.to_string()));
    let Some(parent_name) = parent_name else {
        return Ok(ImportlibParentSearchPathsPayload {
            has_parent: false,
            parent_name: None,
            search_paths: Vec::new(),
            needs_parent_spec: false,
            package_context: false,
        });
    };

    let parent_key_bits = alloc_str_bits(_py, &parent_name)?;
    let parent_bits = match importlib_dict_get_string_key_bits(_py, modules_ptr, parent_key_bits) {
        Ok(value) => value,
        Err(err) => {
            if !obj_from_bits(parent_key_bits).is_none() {
                dec_ref_bits(_py, parent_key_bits);
            }
            return Err(err);
        }
    };
    if let Some(parent_bits) = parent_bits {
        let path_name = intern_static_name(_py, &DUNDER_PATH_NAME, b"__path__");
        let parent_path_bits = match getattr_optional_bits(_py, parent_bits, path_name) {
            Ok(Some(bits)) => bits,
            Ok(None) => MoltObject::none().bits(),
            Err(err) => {
                if !obj_from_bits(parent_key_bits).is_none() {
                    dec_ref_bits(_py, parent_key_bits);
                }
                return Err(err);
            }
        };
        let search_paths = match importlib_coerce_search_paths_values(
            _py,
            parent_path_bits,
            "invalid parent package search path",
        ) {
            Ok(value) => value,
            Err(err) => {
                if !obj_from_bits(parent_path_bits).is_none() {
                    dec_ref_bits(_py, parent_path_bits);
                }
                if !obj_from_bits(parent_key_bits).is_none() {
                    dec_ref_bits(_py, parent_key_bits);
                }
                return Err(err);
            }
        };
        if !obj_from_bits(parent_path_bits).is_none() {
            dec_ref_bits(_py, parent_path_bits);
        }
        if !obj_from_bits(parent_key_bits).is_none() {
            dec_ref_bits(_py, parent_key_bits);
        }
        return Ok(ImportlibParentSearchPathsPayload {
            has_parent: true,
            parent_name: Some(parent_name),
            search_paths,
            needs_parent_spec: false,
            package_context: true,
        });
    }

    if !obj_from_bits(parent_key_bits).is_none() {
        dec_ref_bits(_py, parent_key_bits);
    }
    Ok(ImportlibParentSearchPathsPayload {
        has_parent: true,
        parent_name: Some(parent_name),
        search_paths: Vec::new(),
        needs_parent_spec: true,
        package_context: true,
    })
}

fn importlib_runtime_state_view_bits(
    _py: &PyToken<'_>,
) -> Result<ImportlibRuntimeStateViewBits, u64> {
    static MODULES_NAME: AtomicU64 = AtomicU64::new(0);
    static META_PATH_NAME: AtomicU64 = AtomicU64::new(0);
    static PATH_HOOKS_NAME: AtomicU64 = AtomicU64::new(0);
    static PATH_IMPORTER_CACHE_NAME: AtomicU64 = AtomicU64::new(0);

    let runtime_state_bits = importlib_runtime_state_payload_bits(_py)?;
    let out = (|| -> Result<ImportlibRuntimeStateViewBits, u64> {
        let Some(runtime_state_ptr) = obj_from_bits(runtime_state_bits).as_ptr() else {
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "invalid importlib runtime state payload: dict expected",
            ));
        };
        if unsafe { object_type_id(runtime_state_ptr) } != TYPE_ID_DICT {
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                "invalid importlib runtime state payload: dict expected",
            ));
        }

        let modules_key = intern_static_name(_py, &MODULES_NAME, b"modules");
        let meta_path_key = intern_static_name(_py, &META_PATH_NAME, b"meta_path");
        let path_hooks_key = intern_static_name(_py, &PATH_HOOKS_NAME, b"path_hooks");
        let path_importer_cache_key =
            intern_static_name(_py, &PATH_IMPORTER_CACHE_NAME, b"path_importer_cache");
        let modules_bits = unsafe { dict_get_in_place(_py, runtime_state_ptr, modules_key) }
            .unwrap_or(MoltObject::none().bits());
        let meta_path_bits = unsafe { dict_get_in_place(_py, runtime_state_ptr, meta_path_key) }
            .unwrap_or(MoltObject::none().bits());
        let path_hooks_bits = unsafe { dict_get_in_place(_py, runtime_state_ptr, path_hooks_key) }
            .unwrap_or(MoltObject::none().bits());
        let path_importer_cache_bits =
            unsafe { dict_get_in_place(_py, runtime_state_ptr, path_importer_cache_key) }
                .unwrap_or(MoltObject::none().bits());
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }

        let Some(modules_ptr) = obj_from_bits(modules_bits).as_ptr() else {
            return Err(importlib_modules_runtime_error(_py));
        };
        if unsafe { object_type_id(modules_ptr) } != TYPE_ID_DICT {
            return Err(importlib_modules_runtime_error(_py));
        }
        if !obj_from_bits(path_importer_cache_bits).is_none() {
            let Some(path_importer_cache_ptr) = obj_from_bits(path_importer_cache_bits).as_ptr()
            else {
                return Err(raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "invalid importlib runtime state payload: path_importer_cache",
                ));
            };
            if unsafe { object_type_id(path_importer_cache_ptr) } != TYPE_ID_DICT {
                return Err(raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "invalid importlib runtime state payload: path_importer_cache",
                ));
            }
        }

        for bits in [
            modules_bits,
            meta_path_bits,
            path_hooks_bits,
            path_importer_cache_bits,
        ] {
            if !obj_from_bits(bits).is_none() {
                inc_ref_bits(_py, bits);
            }
        }
        Ok(ImportlibRuntimeStateViewBits {
            modules_bits,
            meta_path_bits,
            path_hooks_bits,
            path_importer_cache_bits,
        })
    })();
    if !obj_from_bits(runtime_state_bits).is_none() {
        dec_ref_bits(_py, runtime_state_bits);
    }
    out
}

fn importlib_lookup_callable_attr(
    _py: &PyToken<'_>,
    target_bits: u64,
    slot: &AtomicU64,
    name: &'static [u8],
) -> Result<Option<u64>, u64> {
    let attr_name = intern_static_name(_py, slot, name);
    importlib_reader_lookup_callable(_py, target_bits, attr_name)
}

fn importlib_clear_mapping_like_best_effort(_py: &PyToken<'_>, mapping_bits: u64) {
    static CLEAR_NAME: AtomicU64 = AtomicU64::new(0);
    if obj_from_bits(mapping_bits).is_none() {
        return;
    }
    if let Some(mapping_ptr) = obj_from_bits(mapping_bits).as_ptr()
        && unsafe { object_type_id(mapping_ptr) } == TYPE_ID_DICT
    {
        unsafe {
            dict_clear_in_place(_py, mapping_ptr);
        }
        if exception_pending(_py) {
            clear_exception(_py);
        }
        return;
    }
    let clear_name = intern_static_name(_py, &CLEAR_NAME, b"clear");
    let clear_bits = match importlib_reader_lookup_callable(_py, mapping_bits, clear_name) {
        Ok(Some(bits)) => bits,
        Ok(None) => return,
        Err(_) => {
            if exception_pending(_py) {
                clear_exception(_py);
            }
            return;
        }
    };
    let result = call_callable_positional(_py, clear_bits, &[]);
    if !obj_from_bits(clear_bits).is_none() {
        dec_ref_bits(_py, clear_bits);
    }
    if let Ok(result_bits) = result
        && !obj_from_bits(result_bits).is_none()
    {
        dec_ref_bits(_py, result_bits);
    }
    if exception_pending(_py) {
        clear_exception(_py);
    }
}

fn importlib_clear_mapping_attr_best_effort(
    _py: &PyToken<'_>,
    target_bits: u64,
    slot: &AtomicU64,
    name: &'static [u8],
) {
    let attr_name = intern_static_name(_py, slot, name);
    let attr_bits = match getattr_optional_bits(_py, target_bits, attr_name) {
        Ok(Some(bits)) => bits,
        Ok(None) => return,
        Err(_) => {
            if exception_pending(_py) {
                clear_exception(_py);
            }
            return;
        }
    };
    importlib_clear_mapping_like_best_effort(_py, attr_bits);
    if !obj_from_bits(attr_bits).is_none() {
        dec_ref_bits(_py, attr_bits);
    }
}

fn importlib_module_cache_lookup_bits(_py: &PyToken<'_>, module_name: &str) -> Option<u64> {
    let module_cache = crate::builtins::exceptions::internals::module_cache(_py);
    let guard = module_cache.lock().unwrap();
    guard.get(module_name).copied()
}

fn importlib_key_starts_with_underscore(key_bits: u64) -> bool {
    let Some(key_ptr) = obj_from_bits(key_bits).as_ptr() else {
        return false;
    };
    if unsafe { object_type_id(key_ptr) } != TYPE_ID_STRING {
        return false;
    }
    let key_len = unsafe { string_len(key_ptr) };
    if key_len == 0 {
        return false;
    }
    let key_bytes = unsafe { std::slice::from_raw_parts(string_bytes(key_ptr), key_len) };
    key_bytes[0] == b'_'
}

fn importlib_module_dict_ptr(module_bits: u64) -> Option<*mut u8> {
    let module_ptr = obj_from_bits(module_bits).as_ptr()?;
    if unsafe { object_type_id(module_ptr) } != TYPE_ID_MODULE {
        return None;
    }
    let dict_bits = unsafe { module_dict_bits(module_ptr) };
    let dict_ptr = obj_from_bits(dict_bits).as_ptr()?;
    if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
        return None;
    }
    Some(dict_ptr)
}

fn importlib_module_name_matches(
    _py: &PyToken<'_>,
    module_name: &str,
    module_bits: u64,
) -> Result<bool, u64> {
    static NAME_NAME: AtomicU64 = AtomicU64::new(0);
    let name_attr = intern_static_name(_py, &NAME_NAME, b"__name__");
    let Some(name_bits) = getattr_optional_bits(_py, module_bits, name_attr)? else {
        return Ok(false);
    };
    let matches = string_obj_to_owned(obj_from_bits(name_bits))
        .map(|value| value == module_name)
        .unwrap_or(false);
    if !obj_from_bits(name_bits).is_none() {
        dec_ref_bits(_py, name_bits);
    }
    Ok(matches)
}

fn importlib_module_public_surface_empty(
    _py: &PyToken<'_>,
    module_name: &str,
    module_bits: u64,
) -> Result<bool, u64> {
    if !importlib_module_name_matches(_py, module_name, module_bits)? {
        return Ok(false);
    }
    let Some(dict_ptr) = importlib_module_dict_ptr(module_bits) else {
        return Ok(false);
    };
    let entries = unsafe { dict_order(dict_ptr) };
    for idx in (0..entries.len()).step_by(2) {
        if !importlib_key_starts_with_underscore(entries[idx]) {
            return Ok(false);
        }
    }
    Ok(true)
}

fn importlib_module_is_empty_placeholder(
    _py: &PyToken<'_>,
    module_name: &str,
    module_bits: u64,
) -> Result<bool, u64> {
    static SPEC_NAME: AtomicU64 = AtomicU64::new(0);
    static FILE_NAME: AtomicU64 = AtomicU64::new(0);
    static LOADER_NAME: AtomicU64 = AtomicU64::new(0);

    if !importlib_module_name_matches(_py, module_name, module_bits)? {
        return Ok(false);
    }
    let Some(dict_ptr) = importlib_module_dict_ptr(module_bits) else {
        return Ok(false);
    };
    let entries = unsafe { dict_order(dict_ptr) };
    for idx in (0..entries.len()).step_by(2) {
        if !importlib_key_starts_with_underscore(entries[idx]) {
            return Ok(false);
        }
    }

    let spec_name = intern_static_name(_py, &SPEC_NAME, b"__spec__");
    let file_name = intern_static_name(_py, &FILE_NAME, b"__file__");
    let loader_name = intern_static_name(_py, &LOADER_NAME, b"loader");
    let spec_bits = importlib_dict_get_string_key_bits(_py, dict_ptr, spec_name)?;
    let file_bits = importlib_dict_get_string_key_bits(_py, dict_ptr, file_name)?;

    let file_is_none = file_bits.is_none();
    let loader_is_none = match spec_bits {
        None => true,
        Some(spec_bits) => {
            let attr = getattr_optional_bits(_py, spec_bits, loader_name)?;
            let loader_bits = attr.unwrap_or_else(|| MoltObject::none().bits());
            let out = obj_from_bits(loader_bits).is_none();
            if !obj_from_bits(loader_bits).is_none() {
                dec_ref_bits(_py, loader_bits);
            }
            out
        }
    };
    Ok(file_is_none && loader_is_none)
}

fn importlib_module_should_retry_empty(
    _py: &PyToken<'_>,
    module_name: &str,
    module_bits: u64,
) -> Result<bool, u64> {
    if !IMPORTLIB_EMPTY_MODULE_RETRY_PREFIXES
        .iter()
        .any(|prefix| module_name.starts_with(prefix))
    {
        return Ok(false);
    }
    importlib_module_public_surface_empty(_py, module_name, module_bits)
}

fn pending_exception_kind_and_message(_py: &PyToken<'_>) -> Option<(String, String)> {
    if !exception_pending(_py) {
        return None;
    }
    let exc_bits = molt_exception_last();
    let Some(exc_ptr) = maybe_ptr_from_bits(exc_bits) else {
        if !obj_from_bits(exc_bits).is_none() {
            dec_ref_bits(_py, exc_bits);
        }
        return None;
    };
    let kind_bits = unsafe { exception_kind_bits(exc_ptr) };
    let Some(kind) = string_obj_to_owned(obj_from_bits(kind_bits)) else {
        if !obj_from_bits(exc_bits).is_none() {
            dec_ref_bits(_py, exc_bits);
        }
        return None;
    };
    let message = format_obj_str(_py, obj_from_bits(exc_bits));
    if !obj_from_bits(exc_bits).is_none() {
        dec_ref_bits(_py, exc_bits);
    }
    Some((kind, message))
}

fn importlib_rethrow_pending_exception(_py: &PyToken<'_>) {
    let Some((kind, message)) = pending_exception_kind_and_message(_py) else {
        return;
    };
    clear_exception(_py);
    let _ = raise_exception::<u64>(_py, &kind, &message);
}

fn importlib_exception_should_fallback(_py: &PyToken<'_>) -> bool {
    let Some((kind, message)) = pending_exception_kind_and_message(_py) else {
        return false;
    };
    if kind == "ImportError" || kind == "ModuleNotFoundError" {
        let is_missing_module = message.starts_with("No module named ")
            || message.contains("No module named '")
            || message.contains("No module named \"");
        if is_missing_module {
            clear_exception(_py);
            return true;
        }
        return false;
    }
    if kind == "TypeError" && message.contains("import returned non-module payload") {
        clear_exception(_py);
        return true;
    }
    false
}

fn importlib_required_callable(
    _py: &PyToken<'_>,
    target_bits: u64,
    name_slot: &AtomicU64,
    name: &'static [u8],
    owner: &str,
) -> Result<u64, u64> {
    let attr_name = intern_static_name(_py, name_slot, name);
    match importlib_reader_lookup_callable(_py, target_bits, attr_name)? {
        Some(bits) => Ok(bits),
        None => Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            &format!("{owner}.{} is unavailable", String::from_utf8_lossy(name)),
        )),
    }
}

fn importlib_required_attribute(
    _py: &PyToken<'_>,
    target_bits: u64,
    name_slot: &AtomicU64,
    name: &'static [u8],
    owner: &str,
) -> Result<u64, u64> {
    let attr_name = intern_static_name(_py, name_slot, name);
    match getattr_optional_bits(_py, target_bits, attr_name)? {
        Some(bits) => Ok(bits),
        None => Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            &format!("{owner}.{} is unavailable", String::from_utf8_lossy(name)),
        )),
    }
}

fn importlib_loader_is_molt_loader(
    _py: &PyToken<'_>,
    loader_bits: u64,
    machinery_bits: u64,
) -> Result<bool, u64> {
    static BUILTIN_IMPORTER_NAME: AtomicU64 = AtomicU64::new(0);
    let attr_name = intern_static_name(_py, &BUILTIN_IMPORTER_NAME, b"BuiltinImporter");
    let Some(loader_cls_bits) = getattr_optional_bits(_py, machinery_bits, attr_name)? else {
        return Ok(false);
    };
    if obj_from_bits(loader_cls_bits).is_none() {
        return Ok(false);
    }
    let is_instance_bits = crate::molt_isinstance(loader_bits, loader_cls_bits);
    dec_ref_bits(_py, loader_cls_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    let out = is_truthy(_py, obj_from_bits(is_instance_bits));
    if !obj_from_bits(is_instance_bits).is_none() {
        dec_ref_bits(_py, is_instance_bits);
    }
    Ok(out)
}

fn importlib_set_attr(
    _py: &PyToken<'_>,
    target_bits: u64,
    slot: &AtomicU64,
    name: &'static [u8],
    value_bits: u64,
) -> Result<(), u64> {
    let attr_name = intern_static_name(_py, slot, name);
    let result_bits = crate::molt_object_setattr(target_bits, attr_name, value_bits);
    if !obj_from_bits(result_bits).is_none() {
        dec_ref_bits(_py, result_bits);
    }
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(())
}

fn importlib_machinery_loader_instance(
    _py: &PyToken<'_>,
    machinery_bits: u64,
    class_slot: &AtomicU64,
    class_name: &'static [u8],
    args: &[u64],
) -> Result<u64, u64> {
    let loader_cls_bits = importlib_required_attribute(
        _py,
        machinery_bits,
        class_slot,
        class_name,
        "importlib.machinery",
    )?;
    let loader_bits = call_callable_positional(_py, loader_cls_bits, args)?;
    dec_ref_bits(_py, loader_cls_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    Ok(loader_bits)
}

fn importlib_machinery_builtin_loader(_py: &PyToken<'_>, machinery_bits: u64) -> Result<u64, u64> {
    static MOLT_LOADER_NAME: AtomicU64 = AtomicU64::new(0);
    static BUILTIN_IMPORTER_NAME: AtomicU64 = AtomicU64::new(0);
    let cache_name = intern_static_name(_py, &MOLT_LOADER_NAME, b"_MOLT_LOADER");
    if let Some(loader_bits) = getattr_optional_bits(_py, machinery_bits, cache_name)?
        && !obj_from_bits(loader_bits).is_none()
    {
        return Ok(loader_bits);
    }
    let loader_bits = importlib_machinery_loader_instance(
        _py,
        machinery_bits,
        &BUILTIN_IMPORTER_NAME,
        b"BuiltinImporter",
        &[],
    )?;
    if let Err(err) = importlib_set_attr(
        _py,
        machinery_bits,
        &MOLT_LOADER_NAME,
        b"_MOLT_LOADER",
        loader_bits,
    ) {
        if !obj_from_bits(loader_bits).is_none() {
            dec_ref_bits(_py, loader_bits);
        }
        return Err(err);
    }
    Ok(loader_bits)
}

fn importlib_find_spec_object_bits(
    _py: &PyToken<'_>,
    fullname: &str,
    payload: &ImportlibFindSpecPayload,
    machinery_bits: u64,
) -> Result<u64, u64> {
    static MODULE_SPEC_NAME: AtomicU64 = AtomicU64::new(0);
    static SOURCE_FILE_LOADER_NAME: AtomicU64 = AtomicU64::new(0);
    static EXTENSION_FILE_LOADER_NAME: AtomicU64 = AtomicU64::new(0);
    static SOURCELESS_FILE_LOADER_NAME: AtomicU64 = AtomicU64::new(0);
    static ZIP_SOURCE_LOADER_NAME: AtomicU64 = AtomicU64::new(0);
    static SUBMODULE_SEARCH_LOCATIONS_NAME: AtomicU64 = AtomicU64::new(0);
    static CACHED_NAME: AtomicU64 = AtomicU64::new(0);
    static HAS_LOCATION_NAME: AtomicU64 = AtomicU64::new(0);

    let fullname_bits = alloc_str_bits(_py, fullname)?;
    let origin_bits = match payload.origin.as_deref() {
        Some(origin) => match alloc_str_bits(_py, origin) {
            Ok(bits) => bits,
            Err(err) => {
                if !obj_from_bits(fullname_bits).is_none() {
                    dec_ref_bits(_py, fullname_bits);
                }
                return Err(err);
            }
        },
        None => MoltObject::none().bits(),
    };
    let loader_bits = match payload.loader_kind.as_str() {
        "builtin" => match importlib_machinery_builtin_loader(_py, machinery_bits) {
            Ok(bits) => bits,
            Err(err) => {
                if !obj_from_bits(fullname_bits).is_none() {
                    dec_ref_bits(_py, fullname_bits);
                }
                if !obj_from_bits(origin_bits).is_none() {
                    dec_ref_bits(_py, origin_bits);
                }
                return Err(err);
            }
        },
        "source" => match importlib_machinery_loader_instance(
            _py,
            machinery_bits,
            &SOURCE_FILE_LOADER_NAME,
            b"SourceFileLoader",
            &[fullname_bits, origin_bits],
        ) {
            Ok(bits) => bits,
            Err(err) => {
                if !obj_from_bits(fullname_bits).is_none() {
                    dec_ref_bits(_py, fullname_bits);
                }
                if !obj_from_bits(origin_bits).is_none() {
                    dec_ref_bits(_py, origin_bits);
                }
                return Err(err);
            }
        },
        "extension" => match importlib_machinery_loader_instance(
            _py,
            machinery_bits,
            &EXTENSION_FILE_LOADER_NAME,
            b"ExtensionFileLoader",
            &[fullname_bits, origin_bits],
        ) {
            Ok(bits) => bits,
            Err(err) => {
                if !obj_from_bits(fullname_bits).is_none() {
                    dec_ref_bits(_py, fullname_bits);
                }
                if !obj_from_bits(origin_bits).is_none() {
                    dec_ref_bits(_py, origin_bits);
                }
                return Err(err);
            }
        },
        "bytecode" => match importlib_machinery_loader_instance(
            _py,
            machinery_bits,
            &SOURCELESS_FILE_LOADER_NAME,
            b"SourcelessFileLoader",
            &[fullname_bits, origin_bits],
        ) {
            Ok(bits) => bits,
            Err(err) => {
                if !obj_from_bits(fullname_bits).is_none() {
                    dec_ref_bits(_py, fullname_bits);
                }
                if !obj_from_bits(origin_bits).is_none() {
                    dec_ref_bits(_py, origin_bits);
                }
                return Err(err);
            }
        },
        "zip_source" => {
            let Some(zip_archive) = payload.zip_archive.as_deref() else {
                if !obj_from_bits(fullname_bits).is_none() {
                    dec_ref_bits(_py, fullname_bits);
                }
                if !obj_from_bits(origin_bits).is_none() {
                    dec_ref_bits(_py, origin_bits);
                }
                return Err(raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "invalid importlib find-spec payload: zip source archive missing",
                ));
            };
            let Some(zip_inner_path) = payload.zip_inner_path.as_deref() else {
                if !obj_from_bits(fullname_bits).is_none() {
                    dec_ref_bits(_py, fullname_bits);
                }
                if !obj_from_bits(origin_bits).is_none() {
                    dec_ref_bits(_py, origin_bits);
                }
                return Err(raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "invalid importlib find-spec payload: zip source inner path missing",
                ));
            };
            let zip_archive_bits = match alloc_str_bits(_py, zip_archive) {
                Ok(bits) => bits,
                Err(err) => {
                    if !obj_from_bits(fullname_bits).is_none() {
                        dec_ref_bits(_py, fullname_bits);
                    }
                    if !obj_from_bits(origin_bits).is_none() {
                        dec_ref_bits(_py, origin_bits);
                    }
                    return Err(err);
                }
            };
            let zip_inner_path_bits = match alloc_str_bits(_py, zip_inner_path) {
                Ok(bits) => bits,
                Err(err) => {
                    if !obj_from_bits(zip_archive_bits).is_none() {
                        dec_ref_bits(_py, zip_archive_bits);
                    }
                    if !obj_from_bits(fullname_bits).is_none() {
                        dec_ref_bits(_py, fullname_bits);
                    }
                    if !obj_from_bits(origin_bits).is_none() {
                        dec_ref_bits(_py, origin_bits);
                    }
                    return Err(err);
                }
            };
            let out = importlib_machinery_loader_instance(
                _py,
                machinery_bits,
                &ZIP_SOURCE_LOADER_NAME,
                b"_ZipSourceLoader",
                &[fullname_bits, zip_archive_bits, zip_inner_path_bits],
            );
            if !obj_from_bits(zip_archive_bits).is_none() {
                dec_ref_bits(_py, zip_archive_bits);
            }
            if !obj_from_bits(zip_inner_path_bits).is_none() {
                dec_ref_bits(_py, zip_inner_path_bits);
            }
            match out {
                Ok(bits) => bits,
                Err(err) => {
                    if !obj_from_bits(fullname_bits).is_none() {
                        dec_ref_bits(_py, fullname_bits);
                    }
                    if !obj_from_bits(origin_bits).is_none() {
                        dec_ref_bits(_py, origin_bits);
                    }
                    return Err(err);
                }
            }
        }
        "namespace" => MoltObject::none().bits(),
        kind => {
            if !obj_from_bits(fullname_bits).is_none() {
                dec_ref_bits(_py, fullname_bits);
            }
            if !obj_from_bits(origin_bits).is_none() {
                dec_ref_bits(_py, origin_bits);
            }
            return Err(raise_exception::<_>(
                _py,
                "RuntimeError",
                &format!("unsupported importlib loader kind: {kind}"),
            ));
        }
    };

    let is_package_bits = MoltObject::from_bool(payload.is_package).bits();
    let module_spec_cls_bits = match importlib_required_attribute(
        _py,
        machinery_bits,
        &MODULE_SPEC_NAME,
        b"ModuleSpec",
        "importlib.machinery",
    ) {
        Ok(bits) => bits,
        Err(err) => {
            if !obj_from_bits(fullname_bits).is_none() {
                dec_ref_bits(_py, fullname_bits);
            }
            if !obj_from_bits(origin_bits).is_none() {
                dec_ref_bits(_py, origin_bits);
            }
            if !obj_from_bits(loader_bits).is_none() {
                dec_ref_bits(_py, loader_bits);
            }
            return Err(err);
        }
    };
    let spec_bits = match call_callable_positional(
        _py,
        module_spec_cls_bits,
        &[fullname_bits, loader_bits, origin_bits, is_package_bits],
    ) {
        Ok(bits) => bits,
        Err(err) => {
            dec_ref_bits(_py, module_spec_cls_bits);
            if !obj_from_bits(fullname_bits).is_none() {
                dec_ref_bits(_py, fullname_bits);
            }
            if !obj_from_bits(origin_bits).is_none() {
                dec_ref_bits(_py, origin_bits);
            }
            if !obj_from_bits(loader_bits).is_none() {
                dec_ref_bits(_py, loader_bits);
            }
            return Err(err);
        }
    };
    dec_ref_bits(_py, module_spec_cls_bits);
    if exception_pending(_py) {
        if !obj_from_bits(fullname_bits).is_none() {
            dec_ref_bits(_py, fullname_bits);
        }
        if !obj_from_bits(origin_bits).is_none() {
            dec_ref_bits(_py, origin_bits);
        }
        if !obj_from_bits(loader_bits).is_none() {
            dec_ref_bits(_py, loader_bits);
        }
        return Err(MoltObject::none().bits());
    }

    if let Some(locations) = payload.submodule_search_locations.as_ref() {
        let Some(locations_bits) = alloc_string_list_bits(_py, locations) else {
            if !obj_from_bits(spec_bits).is_none() {
                dec_ref_bits(_py, spec_bits);
            }
            if !obj_from_bits(fullname_bits).is_none() {
                dec_ref_bits(_py, fullname_bits);
            }
            if !obj_from_bits(origin_bits).is_none() {
                dec_ref_bits(_py, origin_bits);
            }
            if !obj_from_bits(loader_bits).is_none() {
                dec_ref_bits(_py, loader_bits);
            }
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        };
        let out = importlib_set_attr(
            _py,
            spec_bits,
            &SUBMODULE_SEARCH_LOCATIONS_NAME,
            b"submodule_search_locations",
            locations_bits,
        );
        if !obj_from_bits(locations_bits).is_none() {
            dec_ref_bits(_py, locations_bits);
        }
        if let Err(err) = out {
            if !obj_from_bits(spec_bits).is_none() {
                dec_ref_bits(_py, spec_bits);
            }
            if !obj_from_bits(fullname_bits).is_none() {
                dec_ref_bits(_py, fullname_bits);
            }
            if !obj_from_bits(origin_bits).is_none() {
                dec_ref_bits(_py, origin_bits);
            }
            if !obj_from_bits(loader_bits).is_none() {
                dec_ref_bits(_py, loader_bits);
            }
            return Err(err);
        }
    }

    let computed_cached = if let Some(cached) = payload.cached.as_ref() {
        Some(cached.clone())
    } else if payload.loader_kind == "source" {
        payload
            .origin
            .as_ref()
            .map(|origin| importlib_cache_from_source(origin))
    } else {
        None
    };
    let cached_bits = match computed_cached.as_deref() {
        Some(cached) => match alloc_str_bits(_py, cached) {
            Ok(bits) => bits,
            Err(err) => {
                if !obj_from_bits(spec_bits).is_none() {
                    dec_ref_bits(_py, spec_bits);
                }
                if !obj_from_bits(fullname_bits).is_none() {
                    dec_ref_bits(_py, fullname_bits);
                }
                if !obj_from_bits(origin_bits).is_none() {
                    dec_ref_bits(_py, origin_bits);
                }
                if !obj_from_bits(loader_bits).is_none() {
                    dec_ref_bits(_py, loader_bits);
                }
                return Err(err);
            }
        },
        None => MoltObject::none().bits(),
    };
    if let Err(err) = importlib_set_attr(_py, spec_bits, &CACHED_NAME, b"cached", cached_bits) {
        if !obj_from_bits(cached_bits).is_none() {
            dec_ref_bits(_py, cached_bits);
        }
        if !obj_from_bits(spec_bits).is_none() {
            dec_ref_bits(_py, spec_bits);
        }
        if !obj_from_bits(fullname_bits).is_none() {
            dec_ref_bits(_py, fullname_bits);
        }
        if !obj_from_bits(origin_bits).is_none() {
            dec_ref_bits(_py, origin_bits);
        }
        if !obj_from_bits(loader_bits).is_none() {
            dec_ref_bits(_py, loader_bits);
        }
        return Err(err);
    }
    if !obj_from_bits(cached_bits).is_none() {
        dec_ref_bits(_py, cached_bits);
    }

    let has_location_bits = MoltObject::from_bool(payload.has_location).bits();
    if let Err(err) = importlib_set_attr(
        _py,
        spec_bits,
        &HAS_LOCATION_NAME,
        b"has_location",
        has_location_bits,
    ) {
        if !obj_from_bits(spec_bits).is_none() {
            dec_ref_bits(_py, spec_bits);
        }
        if !obj_from_bits(fullname_bits).is_none() {
            dec_ref_bits(_py, fullname_bits);
        }
        if !obj_from_bits(origin_bits).is_none() {
            dec_ref_bits(_py, origin_bits);
        }
        if !obj_from_bits(loader_bits).is_none() {
            dec_ref_bits(_py, loader_bits);
        }
        return Err(err);
    }

    if !obj_from_bits(fullname_bits).is_none() {
        dec_ref_bits(_py, fullname_bits);
    }
    if !obj_from_bits(origin_bits).is_none() {
        dec_ref_bits(_py, origin_bits);
    }
    if !obj_from_bits(loader_bits).is_none() {
        dec_ref_bits(_py, loader_bits);
    }

    Ok(spec_bits)
}

fn importlib_list_from_iterable(
    _py: &PyToken<'_>,
    value_bits: u64,
    name: &str,
) -> Result<u64, u64> {
    let list_bits = unsafe { call_callable1(_py, builtin_classes(_py).list, value_bits) };
    if exception_pending(_py) {
        return Err(raise_exception::<_>(
            _py,
            "TypeError",
            &format!("{name} must be iterable"),
        ));
    }
    Ok(list_bits)
}

fn importlib_is_str_list_bits(bits: u64) -> bool {
    let Some(ptr) = obj_from_bits(bits).as_ptr() else {
        return false;
    };
    unsafe {
        if object_type_id(ptr) != TYPE_ID_LIST {
            return false;
        }
        let values = seq_vec_ref(ptr);
        for &value_bits in values {
            if string_obj_to_owned(obj_from_bits(value_bits)).is_none() {
                return false;
            }
        }
    }
    true
}

fn importlib_module_set_core_state(
    _py: &PyToken<'_>,
    module_bits: u64,
    loader_bits: u64,
    origin_bits: u64,
    module_package_bits: u64,
) -> Result<(), u64> {
    static DUNDER_LOADER_NAME: AtomicU64 = AtomicU64::new(0);
    static DUNDER_FILE_NAME: AtomicU64 = AtomicU64::new(0);
    static DUNDER_CACHED_NAME: AtomicU64 = AtomicU64::new(0);
    static DUNDER_PACKAGE_NAME: AtomicU64 = AtomicU64::new(0);
    importlib_set_attr(
        _py,
        module_bits,
        &DUNDER_LOADER_NAME,
        b"__loader__",
        loader_bits,
    )?;
    importlib_set_attr(
        _py,
        module_bits,
        &DUNDER_FILE_NAME,
        b"__file__",
        origin_bits,
    )?;
    importlib_set_attr(
        _py,
        module_bits,
        &DUNDER_CACHED_NAME,
        b"__cached__",
        MoltObject::none().bits(),
    )?;
    importlib_set_attr(
        _py,
        module_bits,
        &DUNDER_PACKAGE_NAME,
        b"__package__",
        module_package_bits,
    )?;
    Ok(())
}

fn importlib_spec_set_loader_origin(
    _py: &PyToken<'_>,
    spec_bits: u64,
    loader_bits: u64,
    origin_bits: u64,
) -> Result<(), u64> {
    static LOADER_NAME: AtomicU64 = AtomicU64::new(0);
    static ORIGIN_NAME: AtomicU64 = AtomicU64::new(0);
    static HAS_LOCATION_NAME: AtomicU64 = AtomicU64::new(0);
    importlib_set_attr(_py, spec_bits, &LOADER_NAME, b"loader", loader_bits)?;
    importlib_set_attr(_py, spec_bits, &ORIGIN_NAME, b"origin", origin_bits)?;
    importlib_set_attr(
        _py,
        spec_bits,
        &HAS_LOCATION_NAME,
        b"has_location",
        MoltObject::from_bool(true).bits(),
    )?;
    Ok(())
}

fn importlib_single_item_list_bits(_py: &PyToken<'_>, value_bits: u64) -> Result<u64, u64> {
    let list_ptr = alloc_list(_py, &[value_bits]);
    if list_ptr.is_null() {
        return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
    }
    Ok(MoltObject::from_ptr(list_ptr).bits())
}

fn importlib_require_package_root_bits(
    _py: &PyToken<'_>,
    package_root_bits: u64,
) -> Result<(), u64> {
    if obj_from_bits(package_root_bits).is_none()
        || string_obj_to_owned(obj_from_bits(package_root_bits)).is_none()
    {
        return Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            "invalid importlib package root for package module",
        ));
    }
    Ok(())
}

fn importlib_spec_set_cached_from_origin_if_missing(
    _py: &PyToken<'_>,
    spec_bits: u64,
) -> Result<(), u64> {
    static CACHED_NAME: AtomicU64 = AtomicU64::new(0);
    static ORIGIN_NAME: AtomicU64 = AtomicU64::new(0);
    let cached_name = intern_static_name(_py, &CACHED_NAME, b"cached");
    let origin_name = intern_static_name(_py, &ORIGIN_NAME, b"origin");
    let cached_bits = getattr_optional_bits(_py, spec_bits, cached_name)?;
    let should_set = match cached_bits {
        Some(bits) => {
            let out = obj_from_bits(bits).is_none();
            if !obj_from_bits(bits).is_none() {
                dec_ref_bits(_py, bits);
            }
            out
        }
        None => true,
    };
    if !should_set {
        return Ok(());
    }
    let Some(origin_bits) = getattr_optional_bits(_py, spec_bits, origin_name)? else {
        return Ok(());
    };
    let Some(origin) = string_obj_to_owned(obj_from_bits(origin_bits)) else {
        if !obj_from_bits(origin_bits).is_none() {
            dec_ref_bits(_py, origin_bits);
        }
        return Ok(());
    };
    if !obj_from_bits(origin_bits).is_none() {
        dec_ref_bits(_py, origin_bits);
    }
    let cached = importlib_cache_from_source(&origin);
    let cached_bits = alloc_str_bits(_py, &cached)?;
    let out = importlib_set_attr(_py, spec_bits, &CACHED_NAME, b"cached", cached_bits);
    if !obj_from_bits(cached_bits).is_none() {
        dec_ref_bits(_py, cached_bits);
    }
    out
}

fn importlib_import_via_spec(
    _py: &PyToken<'_>,
    resolved: &str,
    resolved_bits: u64,
    modules_ptr: *mut u8,
    util_bits: u64,
    machinery_bits: u64,
) -> Result<u64, u64> {
    static FIND_SPEC_NAME: AtomicU64 = AtomicU64::new(0);
    static MODULE_FROM_SPEC_NAME: AtomicU64 = AtomicU64::new(0);
    static LOADER_NAME: AtomicU64 = AtomicU64::new(0);
    static EXEC_MODULE_NAME: AtomicU64 = AtomicU64::new(0);
    static LOAD_MODULE_NAME: AtomicU64 = AtomicU64::new(0);

    if let Some(existing_bits) =
        importlib_dict_get_string_key_bits(_py, modules_ptr, resolved_bits)?
    {
        inc_ref_bits(_py, existing_bits);
        return Ok(existing_bits);
    }

    let find_spec_bits = importlib_required_callable(
        _py,
        util_bits,
        &FIND_SPEC_NAME,
        b"find_spec",
        "importlib.util",
    )?;
    let spec_bits = unsafe {
        call_callable2(
            _py,
            find_spec_bits,
            resolved_bits,
            MoltObject::none().bits(),
        )
    };
    dec_ref_bits(_py, find_spec_bits);
    if exception_pending(_py) {
        return Err(MoltObject::none().bits());
    }
    if obj_from_bits(spec_bits).is_none() {
        return Err(raise_exception::<_>(
            _py,
            "ModuleNotFoundError",
            &format!("No module named '{resolved}'"),
        ));
    }

    let module_from_spec_bits = importlib_required_callable(
        _py,
        util_bits,
        &MODULE_FROM_SPEC_NAME,
        b"module_from_spec",
        "importlib.util",
    )?;
    let mut module_bits = unsafe { call_callable1(_py, module_from_spec_bits, spec_bits) };
    dec_ref_bits(_py, module_from_spec_bits);
    if exception_pending(_py) {
        if !obj_from_bits(spec_bits).is_none() {
            dec_ref_bits(_py, spec_bits);
        }
        return Err(MoltObject::none().bits());
    }

    let loader_name = intern_static_name(_py, &LOADER_NAME, b"loader");
    let loader_attr = match getattr_optional_bits(_py, spec_bits, loader_name) {
        Ok(value) => value,
        Err(err) => {
            if !obj_from_bits(spec_bits).is_none() {
                dec_ref_bits(_py, spec_bits);
            }
            if !obj_from_bits(module_bits).is_none() {
                dec_ref_bits(_py, module_bits);
            }
            return Err(err);
        }
    };
    let loader_bits = loader_attr.unwrap_or_else(|| MoltObject::none().bits());
    let loader_present = !obj_from_bits(loader_bits).is_none();

    let mut preseed_modules = true;
    if loader_present {
        match importlib_loader_is_molt_loader(_py, loader_bits, machinery_bits) {
            Ok(is_molt_loader) => {
                if is_molt_loader {
                    preseed_modules = false;
                }
            }
            Err(err) => {
                if !obj_from_bits(loader_bits).is_none() {
                    dec_ref_bits(_py, loader_bits);
                }
                if !obj_from_bits(spec_bits).is_none() {
                    dec_ref_bits(_py, spec_bits);
                }
                if !obj_from_bits(module_bits).is_none() {
                    dec_ref_bits(_py, module_bits);
                }
                return Err(err);
            }
        }
    }

    if preseed_modules {
        unsafe {
            dict_set_in_place(_py, modules_ptr, resolved_bits, module_bits);
        }
        if exception_pending(_py) {
            if !obj_from_bits(loader_bits).is_none() {
                dec_ref_bits(_py, loader_bits);
            }
            if !obj_from_bits(spec_bits).is_none() {
                dec_ref_bits(_py, spec_bits);
            }
            if !obj_from_bits(module_bits).is_none() {
                dec_ref_bits(_py, module_bits);
            }
            return Err(MoltObject::none().bits());
        }
    }

    if loader_present {
        let exec_name = intern_static_name(_py, &EXEC_MODULE_NAME, b"exec_module");
        let load_name = intern_static_name(_py, &LOAD_MODULE_NAME, b"load_module");
        if let Some(exec_bits) = importlib_reader_lookup_callable(_py, loader_bits, exec_name)? {
            let out_bits = unsafe { call_callable1(_py, exec_bits, module_bits) };
            dec_ref_bits(_py, exec_bits);
            if exception_pending(_py) {
                importlib_dict_del_string_key(_py, modules_ptr, resolved_bits);
                if !obj_from_bits(out_bits).is_none() {
                    dec_ref_bits(_py, out_bits);
                }
                if !obj_from_bits(loader_bits).is_none() {
                    dec_ref_bits(_py, loader_bits);
                }
                if !obj_from_bits(spec_bits).is_none() {
                    dec_ref_bits(_py, spec_bits);
                }
                if !obj_from_bits(module_bits).is_none() {
                    dec_ref_bits(_py, module_bits);
                }
                return Err(MoltObject::none().bits());
            }
            if !obj_from_bits(out_bits).is_none() {
                dec_ref_bits(_py, out_bits);
            }
        } else if let Some(load_bits) =
            importlib_reader_lookup_callable(_py, loader_bits, load_name)?
        {
            let loaded_bits = unsafe { call_callable1(_py, load_bits, resolved_bits) };
            dec_ref_bits(_py, load_bits);
            if exception_pending(_py) {
                importlib_dict_del_string_key(_py, modules_ptr, resolved_bits);
                if !obj_from_bits(loader_bits).is_none() {
                    dec_ref_bits(_py, loader_bits);
                }
                if !obj_from_bits(spec_bits).is_none() {
                    dec_ref_bits(_py, spec_bits);
                }
                if !obj_from_bits(module_bits).is_none() {
                    dec_ref_bits(_py, module_bits);
                }
                return Err(MoltObject::none().bits());
            }
            if !obj_from_bits(loaded_bits).is_none() {
                if !obj_from_bits(module_bits).is_none() {
                    dec_ref_bits(_py, module_bits);
                }
                module_bits = loaded_bits;
            }
        }
        dec_ref_bits(_py, loader_bits);
    }

    if !preseed_modules {
        unsafe {
            dict_set_in_place(_py, modules_ptr, resolved_bits, module_bits);
        }
        if exception_pending(_py) {
            importlib_dict_del_string_key(_py, modules_ptr, resolved_bits);
            if !obj_from_bits(spec_bits).is_none() {
                dec_ref_bits(_py, spec_bits);
            }
            if !obj_from_bits(module_bits).is_none() {
                dec_ref_bits(_py, module_bits);
            }
            return Err(MoltObject::none().bits());
        }
    }

    let out_bits = match importlib_dict_get_string_key_bits(_py, modules_ptr, resolved_bits)? {
        Some(bits) => {
            inc_ref_bits(_py, bits);
            bits
        }
        None => module_bits,
    };
    if out_bits != module_bits && !obj_from_bits(module_bits).is_none() {
        dec_ref_bits(_py, module_bits);
    }
    if !obj_from_bits(spec_bits).is_none() {
        dec_ref_bits(_py, spec_bits);
    }
    Ok(out_bits)
}

fn importlib_import_with_fallback(
    _py: &PyToken<'_>,
    resolved: &str,
    resolved_bits: u64,
    modules_ptr: *mut u8,
    util_bits: u64,
    machinery_bits: u64,
) -> Result<u64, u64> {
    if IMPORTLIB_SPEC_FIRST_IMPORTS.contains(&resolved) {
        return importlib_import_via_spec(
            _py,
            resolved,
            resolved_bits,
            modules_ptr,
            util_bits,
            machinery_bits,
        );
    }

    let module_bits = crate::molt_module_import(resolved_bits);
    if exception_pending(_py) {
        if importlib_exception_should_fallback(_py) {
            if !obj_from_bits(module_bits).is_none() {
                dec_ref_bits(_py, module_bits);
            }
            return importlib_import_via_spec(
                _py,
                resolved,
                resolved_bits,
                modules_ptr,
                util_bits,
                machinery_bits,
            );
        }
        // Exceptions raised while importing in another runtime lane can carry
        // non-canonical class identities; rethrow in the current lane so
        // Python-level try/except matching uses the local hierarchy.
        importlib_rethrow_pending_exception(_py);
        return Err(MoltObject::none().bits());
    }

    if !obj_from_bits(module_bits).is_none() {
        let should_retry = importlib_module_is_empty_placeholder(_py, resolved, module_bits)?
            || importlib_module_should_retry_empty(_py, resolved, module_bits)?;
        if should_retry {
            clear_exception(_py);
            dec_ref_bits(_py, module_bits);
            return importlib_import_via_spec(
                _py,
                resolved,
                resolved_bits,
                modules_ptr,
                util_bits,
                machinery_bits,
            );
        }
        return Ok(module_bits);
    }

    clear_exception(_py);
    importlib_import_via_spec(
        _py,
        resolved,
        resolved_bits,
        modules_ptr,
        util_bits,
        machinery_bits,
    )
}

fn importlib_bind_submodule_on_parent(
    _py: &PyToken<'_>,
    resolved: &str,
    module_bits: u64,
    modules_ptr: *mut u8,
) -> Result<(), u64> {
    if obj_from_bits(module_bits).is_none() {
        return Ok(());
    }
    let Some((parent_name, child_name)) = resolved.rsplit_once('.') else {
        return Ok(());
    };
    if parent_name.is_empty() || child_name.is_empty() {
        return Ok(());
    }

    let parent_key_bits = alloc_str_bits(_py, parent_name)?;
    let parent_bits = match importlib_dict_get_string_key_bits(_py, modules_ptr, parent_key_bits)? {
        Some(bits) => bits,
        None => {
            if !obj_from_bits(parent_key_bits).is_none() {
                dec_ref_bits(_py, parent_key_bits);
            }
            return Ok(());
        }
    };
    if !obj_from_bits(parent_key_bits).is_none() {
        dec_ref_bits(_py, parent_key_bits);
    }
    let child_name_bits = alloc_str_bits(_py, child_name)?;
    let _ = molt_object_setattr(parent_bits, child_name_bits, module_bits);
    if !obj_from_bits(child_name_bits).is_none() {
        dec_ref_bits(_py, child_name_bits);
    }
    // CPython binds submodules on parents as best-effort metadata; if a parent
    // rejects setattr, keep import success and suppress the side-effect error.
    if exception_pending(_py) {
        clear_exception(_py);
    }
    Ok(())
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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
        match importlib_loader_resolution_payload_bits(_py, &resolution) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_extension_loader_payload(
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
        match importlib_path_is_file(_py, &path) {
            Ok(true) => {}
            Ok(false) => {
                return raise_exception::<_>(
                    _py,
                    "ImportError",
                    "extension module path must point to a file",
                );
            }
            Err(bits) => return bits,
        }
        if let Err(bits) = importlib_require_extension_metadata(_py, &module_name, &path) {
            return bits;
        }
        let spec_is_package = is_truthy(_py, obj_from_bits(spec_is_package_bits));
        let resolution = extension_loader_resolution(&module_name, &path, spec_is_package);
        match importlib_loader_resolution_payload_bits(_py, &resolution) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_sourceless_loader_payload(
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
        let resolution = sourceless_loader_resolution(&module_name, &path, spec_is_package);
        match importlib_loader_resolution_payload_bits(_py, &resolution) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_zip_source_exec_payload(
    module_name_bits: u64,
    archive_path_bits: u64,
    inner_path_bits: u64,
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
        let archive_path = match string_arg_from_bits(_py, archive_path_bits, "archive path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let inner_path = match string_arg_from_bits(_py, inner_path_bits, "inner path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let spec_is_package = is_truthy(_py, obj_from_bits(spec_is_package_bits));
        let payload = match importlib_zip_source_exec_payload(
            &module_name,
            &archive_path,
            &inner_path,
            spec_is_package,
        ) {
            Ok(value) => value,
            Err(err) => return raise_importlib_io_error(_py, err),
        };

        let source_ptr = alloc_string(_py, &payload.source);
        if source_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        let source_bits = MoltObject::from_ptr(source_ptr).bits();
        let origin_bits = match alloc_str_bits(_py, &payload.origin) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, source_bits);
                return err;
            }
        };
        let module_package_bits = match alloc_str_bits(_py, &payload.module_package) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, source_bits);
                dec_ref_bits(_py, origin_bits);
                return err;
            }
        };
        let package_root_bits = match payload.package_root.as_deref() {
            Some(root) => match alloc_str_bits(_py, root) {
                Ok(bits) => bits,
                Err(err) => {
                    dec_ref_bits(_py, source_bits);
                    dec_ref_bits(_py, origin_bits);
                    dec_ref_bits(_py, module_package_bits);
                    return err;
                }
            },
            None => MoltObject::none().bits(),
        };
        let is_package_bits = MoltObject::from_bool(payload.is_package).bits();

        let keys_and_values: [(&[u8], u64); 5] = [
            (b"source", source_bits),
            (b"origin", origin_bits),
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_exec_extension(
    namespace_bits: u64,
    module_name_bits: u64,
    path_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let namespace_ptr = match obj_from_bits(namespace_bits).as_ptr() {
            Some(ptr) if unsafe { object_type_id(ptr) == TYPE_ID_DICT } => ptr,
            _ => return raise_exception::<_>(_py, "TypeError", "namespace must be dict"),
        };
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match importlib_path_is_file(_py, &path) {
            Ok(true) => {}
            Ok(false) => {
                return raise_exception::<_>(
                    _py,
                    "ImportError",
                    "extension module path must point to a file",
                );
            }
            Err(bits) => return bits,
        }
        if !has_capability(_py, "module.extension.exec") && !has_capability(_py, "module.exec") {
            return raise_exception::<_>(
                _py,
                "PermissionError",
                "missing module.extension.exec capability",
            );
        }
        if let Err(bits) = importlib_require_extension_metadata(_py, &module_name, &path) {
            return bits;
        }
        let shim_candidates = importlib_extension_shim_candidates(&module_name, &path);
        let mut restricted_error: Option<String> = None;
        for candidate in &shim_candidates {
            match importlib_path_is_file(_py, candidate) {
                Ok(true) => {
                    if let Err(err) =
                        importlib_exec_restricted_source_path(_py, namespace_ptr, candidate)
                    {
                        if let Some(message) = importlib_restricted_exec_error_message(
                            _py,
                            "extension",
                            &module_name,
                            candidate,
                        ) {
                            if restricted_error.is_none() {
                                restricted_error = Some(message);
                            }
                            continue;
                        }
                        return err;
                    }
                    return MoltObject::none().bits();
                }
                Ok(false) => continue,
                Err(bits) => return bits,
            }
        }
        if let Some(message) = restricted_error {
            return raise_exception::<_>(_py, "ImportError", &message);
        }
        importlib_extension_exec_unavailable(
            _py,
            &module_name,
            &path,
            "extension",
            &shim_candidates,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_exec_sourceless(
    namespace_bits: u64,
    module_name_bits: u64,
    path_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let namespace_ptr = match obj_from_bits(namespace_bits).as_ptr() {
            Some(ptr) if unsafe { object_type_id(ptr) == TYPE_ID_DICT } => ptr,
            _ => return raise_exception::<_>(_py, "TypeError", "namespace must be dict"),
        };
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match importlib_path_is_file(_py, &path) {
            Ok(true) => {}
            Ok(false) => {
                return raise_exception::<_>(
                    _py,
                    "ImportError",
                    "sourceless module path must point to a file",
                );
            }
            Err(bits) => return bits,
        }
        if !has_capability(_py, "module.bytecode.exec") && !has_capability(_py, "module.exec") {
            return raise_exception::<_>(
                _py,
                "PermissionError",
                "missing module.bytecode.exec capability",
            );
        }
        let source_candidates = importlib_sourceless_source_candidates(&module_name, &path);
        let mut restricted_error: Option<String> = None;
        for candidate in &source_candidates {
            match importlib_path_is_file(_py, candidate) {
                Ok(true) => {
                    if let Err(err) =
                        importlib_exec_restricted_source_path(_py, namespace_ptr, candidate)
                    {
                        if let Some(message) = importlib_restricted_exec_error_message(
                            _py,
                            "sourceless",
                            &module_name,
                            candidate,
                        ) {
                            if restricted_error.is_none() {
                                restricted_error = Some(message);
                            }
                            continue;
                        }
                        return err;
                    }
                    return MoltObject::none().bits();
                }
                Ok(false) => continue,
                Err(bits) => return bits,
            }
        }
        if let Some(message) = restricted_error {
            return raise_exception::<_>(_py, "ImportError", &message);
        }
        importlib_extension_exec_unavailable(
            _py,
            &module_name,
            &path,
            "sourceless",
            &source_candidates,
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_linecache_loader_get_source(loader_bits: u64, module_name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match linecache_loader_get_source_impl(_py, loader_bits, &module_name) {
            Ok(Some(source)) => match alloc_str_bits(_py, &source) {
                Ok(bits) => bits,
                Err(err) => err,
            },
            Ok(None) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_module_spec_is_package(module_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        match importlib_module_spec_is_package_bits(_py, module_bits) {
            Ok(value) => MoltObject::from_bool(value).bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_coerce_module_name(
    module_bits: u64,
    loader_bits: u64,
    spec_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        static MODULE_NAME_NAME: AtomicU64 = AtomicU64::new(0);
        static SPEC_NAME: AtomicU64 = AtomicU64::new(0);
        static LOADER_NAME: AtomicU64 = AtomicU64::new(0);

        let module_name_name = intern_static_name(_py, &MODULE_NAME_NAME, b"__name__");
        if let Some(module_name_bits) =
            match getattr_optional_bits(_py, module_bits, module_name_name) {
                Ok(value) => value,
                Err(bits) => return bits,
            }
        {
            if string_obj_to_owned(obj_from_bits(module_name_bits)).is_some() {
                return module_name_bits;
            }
            if !obj_from_bits(module_name_bits).is_none() {
                dec_ref_bits(_py, module_name_bits);
            }
        }

        let mut module_spec_bits = spec_bits;
        let mut module_spec_owned = false;
        if obj_from_bits(module_spec_bits).is_none() {
            let spec_name = intern_static_name(_py, &SPEC_NAME, b"__spec__");
            if let Some(bits) = match getattr_optional_bits(_py, module_bits, spec_name) {
                Ok(value) => value,
                Err(err) => return err,
            } {
                module_spec_bits = bits;
                module_spec_owned = true;
            }
        }

        if !obj_from_bits(module_spec_bits).is_none()
            && let Some(spec_name_bits) =
                match getattr_optional_bits(_py, module_spec_bits, module_name_name) {
                    Ok(value) => value,
                    Err(bits) => {
                        if module_spec_owned {
                            dec_ref_bits(_py, module_spec_bits);
                        }
                        return bits;
                    }
                }
        {
            if string_obj_to_owned(obj_from_bits(spec_name_bits)).is_some() {
                let set_bits =
                    crate::molt_object_setattr(module_bits, module_name_name, spec_name_bits);
                if !obj_from_bits(set_bits).is_none() {
                    dec_ref_bits(_py, set_bits);
                }
                if exception_pending(_py) {
                    clear_exception(_py);
                }
                if module_spec_owned {
                    dec_ref_bits(_py, module_spec_bits);
                }
                return spec_name_bits;
            }
            if !obj_from_bits(spec_name_bits).is_none() {
                dec_ref_bits(_py, spec_name_bits);
            }
        }

        if module_spec_owned && !obj_from_bits(module_spec_bits).is_none() {
            dec_ref_bits(_py, module_spec_bits);
        }

        let loader_name = intern_static_name(_py, &LOADER_NAME, b"name");
        if let Some(loader_name_bits) = match getattr_optional_bits(_py, loader_bits, loader_name) {
            Ok(value) => value,
            Err(bits) => return bits,
        } {
            if string_obj_to_owned(obj_from_bits(loader_name_bits)).is_some() {
                let set_bits =
                    crate::molt_object_setattr(module_bits, module_name_name, loader_name_bits);
                if !obj_from_bits(set_bits).is_none() {
                    dec_ref_bits(_py, set_bits);
                }
                if exception_pending(_py) {
                    clear_exception(_py);
                }
                return loader_name_bits;
            }
            if !obj_from_bits(loader_name_bits).is_none() {
                dec_ref_bits(_py, loader_name_bits);
            }
        }

        raise_exception::<_>(_py, "TypeError", "module name must be str")
    })
}

fn importlib_coerce_search_paths_values(
    _py: &PyToken<'_>,
    value_bits: u64,
    label: &str,
) -> Result<Vec<String>, u64> {
    let mut paths: Vec<String> = Vec::new();
    if obj_from_bits(value_bits).is_none() {
        // value is None -> ()
    } else if let Some(text) = string_obj_to_owned(obj_from_bits(value_bits)) {
        if !text.is_empty() {
            paths.push(text);
        }
    } else {
        let iter_bits = molt_iter(value_bits);
        if exception_pending(_py) {
            clear_exception(_py);
            return Err(raise_exception::<_>(_py, "RuntimeError", label));
        }
        loop {
            let pair_bits = molt_iter_next(iter_bits);
            let Some(pair_ptr) = maybe_ptr_from_bits(pair_bits) else {
                clear_exception(_py);
                return Err(raise_exception::<_>(_py, "RuntimeError", label));
            };
            let pair = unsafe {
                if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                    clear_exception(_py);
                    return Err(raise_exception::<_>(_py, "RuntimeError", label));
                }
                seq_vec_ref(pair_ptr)
            };
            if pair.len() < 2 {
                clear_exception(_py);
                return Err(raise_exception::<_>(_py, "RuntimeError", label));
            }
            if is_truthy(_py, obj_from_bits(pair[1])) {
                break;
            }
            let text_bits = unsafe { call_callable1(_py, builtin_classes(_py).str, pair[0]) };
            if exception_pending(_py) {
                clear_exception(_py);
                if !obj_from_bits(text_bits).is_none() {
                    dec_ref_bits(_py, text_bits);
                }
                return Err(raise_exception::<_>(_py, "RuntimeError", label));
            }
            let Some(text) = string_obj_to_owned(obj_from_bits(text_bits)) else {
                if !obj_from_bits(text_bits).is_none() {
                    dec_ref_bits(_py, text_bits);
                }
                clear_exception(_py);
                return Err(raise_exception::<_>(_py, "RuntimeError", label));
            };
            if !obj_from_bits(text_bits).is_none() {
                dec_ref_bits(_py, text_bits);
            }
            if !text.is_empty() {
                paths.push(text);
            }
        }
    }
    Ok(paths)
}

fn importlib_alloc_string_tuple_bits(_py: &PyToken<'_>, values: &[String]) -> Result<u64, u64> {
    let mut value_bits_vec: Vec<u64> = Vec::with_capacity(values.len());
    for value in values {
        let bits = match alloc_str_bits(_py, value) {
            Ok(bits) => bits,
            Err(err) => {
                for owned in value_bits_vec {
                    if !obj_from_bits(owned).is_none() {
                        dec_ref_bits(_py, owned);
                    }
                }
                return Err(err);
            }
        };
        value_bits_vec.push(bits);
    }
    let tuple_ptr = alloc_tuple(_py, &value_bits_vec);
    for owned in value_bits_vec {
        if !obj_from_bits(owned).is_none() {
            dec_ref_bits(_py, owned);
        }
    }
    if tuple_ptr.is_null() {
        Err(raise_exception::<_>(_py, "MemoryError", "out of memory"))
    } else {
        Ok(MoltObject::from_ptr(tuple_ptr).bits())
    }
}

fn importlib_finder_signature_tuple_bits(
    _py: &PyToken<'_>,
    finders_bits: u64,
    label: &str,
) -> Result<u64, u64> {
    let mut ids: Vec<u64> = Vec::new();
    let iter_bits = molt_iter(finders_bits);
    if exception_pending(_py) {
        clear_exception(_py);
        return Err(raise_exception::<_>(_py, "RuntimeError", label));
    }
    loop {
        let pair_bits = molt_iter_next(iter_bits);
        let Some(pair_ptr) = maybe_ptr_from_bits(pair_bits) else {
            clear_exception(_py);
            return Err(raise_exception::<_>(_py, "RuntimeError", label));
        };
        let pair = unsafe {
            if object_type_id(pair_ptr) != TYPE_ID_TUPLE {
                clear_exception(_py);
                return Err(raise_exception::<_>(_py, "RuntimeError", label));
            }
            seq_vec_ref(pair_ptr)
        };
        if pair.len() < 2 {
            clear_exception(_py);
            return Err(raise_exception::<_>(_py, "RuntimeError", label));
        }
        if is_truthy(_py, obj_from_bits(pair[1])) {
            break;
        }
        let id_bits = crate::molt_id(pair[0]);
        if exception_pending(_py) {
            clear_exception(_py);
            if !obj_from_bits(id_bits).is_none() {
                dec_ref_bits(_py, id_bits);
            }
            for owned in ids {
                if !obj_from_bits(owned).is_none() {
                    dec_ref_bits(_py, owned);
                }
            }
            return Err(raise_exception::<_>(_py, "RuntimeError", label));
        }
        ids.push(id_bits);
    }
    let tuple_ptr = alloc_tuple(_py, &ids);
    for owned in ids {
        if !obj_from_bits(owned).is_none() {
            dec_ref_bits(_py, owned);
        }
    }
    if tuple_ptr.is_null() {
        Err(raise_exception::<_>(_py, "MemoryError", "out of memory"))
    } else {
        Ok(MoltObject::from_ptr(tuple_ptr).bits())
    }
}

fn importlib_path_importer_cache_signature_tuple_bits(
    _py: &PyToken<'_>,
    path_importer_cache_bits: u64,
    label: &str,
) -> Result<u64, u64> {
    if obj_from_bits(path_importer_cache_bits).is_none() {
        let empty_ptr = alloc_tuple(_py, &[]);
        if empty_ptr.is_null() {
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        return Ok(MoltObject::from_ptr(empty_ptr).bits());
    }

    let Some(cache_ptr) = obj_from_bits(path_importer_cache_bits).as_ptr() else {
        return Err(raise_exception::<_>(_py, "RuntimeError", label));
    };
    if unsafe { object_type_id(cache_ptr) } != TYPE_ID_DICT {
        return Err(raise_exception::<_>(_py, "RuntimeError", label));
    }

    let mut pairs: Vec<(String, u64)> = Vec::new();
    let entries = unsafe { dict_order(cache_ptr) };
    for idx in (0..entries.len()).step_by(2) {
        let key_bits = entries[idx];
        let value_bits = entries[idx + 1];
        let Some(key) = string_obj_to_owned(obj_from_bits(key_bits)) else {
            continue;
        };
        let id_bits = crate::molt_id(value_bits);
        if exception_pending(_py) {
            clear_exception(_py);
            if !obj_from_bits(id_bits).is_none() {
                dec_ref_bits(_py, id_bits);
            }
            for (_k, owned) in pairs {
                if !obj_from_bits(owned).is_none() {
                    dec_ref_bits(_py, owned);
                }
            }
            return Err(raise_exception::<_>(_py, "RuntimeError", label));
        }
        pairs.push((key, id_bits));
    }
    pairs.sort_by(|lhs, rhs| lhs.0.cmp(&rhs.0));

    let mut pair_tuple_bits: Vec<u64> = Vec::with_capacity(pairs.len());
    for (key, id_bits) in pairs {
        let key_bits = match alloc_str_bits(_py, &key) {
            Ok(bits) => bits,
            Err(err) => {
                if !obj_from_bits(id_bits).is_none() {
                    dec_ref_bits(_py, id_bits);
                }
                for owned in pair_tuple_bits {
                    if !obj_from_bits(owned).is_none() {
                        dec_ref_bits(_py, owned);
                    }
                }
                return Err(err);
            }
        };
        let item_ptr = alloc_tuple(_py, &[key_bits, id_bits]);
        if !obj_from_bits(key_bits).is_none() {
            dec_ref_bits(_py, key_bits);
        }
        if !obj_from_bits(id_bits).is_none() {
            dec_ref_bits(_py, id_bits);
        }
        if item_ptr.is_null() {
            for owned in pair_tuple_bits {
                if !obj_from_bits(owned).is_none() {
                    dec_ref_bits(_py, owned);
                }
            }
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        pair_tuple_bits.push(MoltObject::from_ptr(item_ptr).bits());
    }
    let out_ptr = alloc_tuple(_py, &pair_tuple_bits);
    for owned in pair_tuple_bits {
        if !obj_from_bits(owned).is_none() {
            dec_ref_bits(_py, owned);
        }
    }
    if out_ptr.is_null() {
        Err(raise_exception::<_>(_py, "MemoryError", "out of memory"))
    } else {
        Ok(MoltObject::from_ptr(out_ptr).bits())
    }
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_coerce_search_paths(value_bits: u64, label_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let label = match string_arg_from_bits(_py, label_bits, "label") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let paths = match importlib_coerce_search_paths_values(_py, value_bits, &label) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match importlib_alloc_string_tuple_bits(_py, &paths) {
            Ok(bits) => bits,
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_finder_signature(finders_bits: u64, label_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let label = match string_arg_from_bits(_py, label_bits, "label") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match importlib_finder_signature_tuple_bits(_py, finders_bits, &label) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_path_importer_cache_signature(
    path_importer_cache_bits: u64,
    label_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let label = match string_arg_from_bits(_py, label_bits, "label") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match importlib_path_importer_cache_signature_tuple_bits(
            _py,
            path_importer_cache_bits,
            &label,
        ) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_path_is_archive_member(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        MoltObject::from_bool(importlib_is_archive_member_path(&path)).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_package_root_from_origin(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match importlib_package_root_from_origin(&path) {
            Some(root) => match alloc_str_bits(_py, &root) {
                Ok(bits) => bits,
                Err(err) => err,
            },
            None => MoltObject::none().bits(),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_validate_resource_name(resource_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let resource = match string_arg_from_bits(_py, resource_bits, "resource name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if let Err(err) = importlib_validate_resource_name_text(_py, &resource) {
            return err;
        }
        match alloc_str_bits(_py, &resource) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_normalize_path(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let text_bits = unsafe { call_callable1(_py, builtin_classes(_py).str, path_bits) };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let Some(path) = string_obj_to_owned(obj_from_bits(text_bits)) else {
            if !obj_from_bits(text_bits).is_none() {
                dec_ref_bits(_py, text_bits);
            }
            return raise_exception::<_>(_py, "TypeError", "path must be str-like");
        };
        if !obj_from_bits(text_bits).is_none() {
            dec_ref_bits(_py, text_bits);
        }
        if let Err(err) = importlib_validate_resource_name_text(_py, &path) {
            return err;
        }
        match alloc_str_bits(_py, &path) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_only(
    iterable_bits: u64,
    default_bits: u64,
    too_long_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let iter_bits = molt_iter(iterable_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let Some(first_bits) = (match importlib_iter_next_value_bits(_py, iter_bits) {
            Ok(value) => value,
            Err(err) => return err,
        }) else {
            inc_ref_bits(_py, default_bits);
            return default_bits;
        };
        let second_value = match importlib_iter_next_value_bits(_py, iter_bits) {
            Ok(value) => value,
            Err(err) => {
                if !obj_from_bits(first_bits).is_none() {
                    dec_ref_bits(_py, first_bits);
                }
                return err;
            }
        };
        let Some(second_bits) = second_value else {
            return first_bits;
        };
        let first_text = importlib_best_effort_str(_py, first_bits);
        let second_text = importlib_best_effort_str(_py, second_bits);
        if !obj_from_bits(first_bits).is_none() {
            dec_ref_bits(_py, first_bits);
        }
        if !obj_from_bits(second_bits).is_none() {
            dec_ref_bits(_py, second_bits);
        }
        let message = format!(
            "Expected exactly one item in iterable, but got {:?}, {:?}, and perhaps more.",
            first_text, second_text
        );
        if !obj_from_bits(too_long_bits).is_none()
            && let Some(kind) = importlib_exception_name_from_bits(_py, too_long_bits)
        {
            return raise_exception::<_>(_py, &kind, &message);
        }
        raise_exception::<_>(_py, "ValueError", &message)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_reader_resource_path_from_roots(
    roots_bits: u64,
    resource_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let roots = match string_sequence_arg_from_bits(_py, roots_bits, "roots") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let resource = match string_arg_from_bits(_py, resource_bits, "resource name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if let Err(err) = importlib_validate_resource_name_text(_py, &resource) {
            return err;
        }
        if let Some(candidate) = importlib_resources_first_fs_file_candidate(&roots, &resource) {
            return match alloc_str_bits(_py, &candidate) {
                Ok(bits) => bits,
                Err(err) => err,
            };
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_reader_open_resource_bytes_from_roots(
    roots_bits: u64,
    resource_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let roots = match string_sequence_arg_from_bits(_py, roots_bits, "roots") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let resource = match string_arg_from_bits(_py, resource_bits, "resource name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if let Err(err) = importlib_validate_resource_name_text(_py, &resource) {
            return err;
        }
        if let Some(candidate) = importlib_resources_first_file_candidate(&roots, &resource) {
            let bytes = match importlib_read_file_bytes(_py, &candidate) {
                Ok(value) => value,
                Err(err) => return err,
            };
            let out_ptr = alloc_bytes(_py, &bytes);
            if out_ptr.is_null() {
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            return MoltObject::from_ptr(out_ptr).bits();
        }
        raise_exception::<_>(_py, "FileNotFoundError", &resource)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_reader_is_resource_from_roots(
    roots_bits: u64,
    resource_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let roots = match string_sequence_arg_from_bits(_py, roots_bits, "roots") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let resource = match string_arg_from_bits(_py, resource_bits, "resource name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if let Err(err) = importlib_validate_resource_name_text(_py, &resource) {
            return err;
        }
        MoltObject::from_bool(importlib_resources_first_file_candidate(&roots, &resource).is_some())
            .bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_reader_contents_from_roots(roots_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let roots = match string_sequence_arg_from_bits(_py, roots_bits, "roots") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut entries: BTreeSet<String> = BTreeSet::new();
        for root in roots {
            let payload = importlib_resources_path_payload(&root);
            for entry in payload.entries {
                entries.insert(entry);
            }
        }
        let out: Vec<String> = entries.into_iter().collect();
        match alloc_string_list_bits(_py, &out) {
            Some(bits) => bits,
            None => raise_exception::<_>(_py, "MemoryError", "out of memory"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_traceback_exception_suppress_context(value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        match traceback_exception_suppress_context_bits(_py, value_bits) {
            Ok(value) => MoltObject::from_bool(value).bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_zip_read_entry(
    archive_path_bits: u64,
    inner_path_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let archive_path = match string_arg_from_bits(_py, archive_path_bits, "archive path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let inner_path = match string_arg_from_bits(_py, inner_path_bits, "inner path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let normalized = inner_path.replace('\\', "/").trim_matches('/').to_string();
        if normalized.is_empty() {
            return raise_exception::<_>(_py, "OSError", "zip archive entry path is empty");
        }
        let bytes = match zip_archive_read_entry(&archive_path, &normalized) {
            Ok(value) => value,
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_read_file(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let bytes = match importlib_read_file_bytes(_py, &path) {
            Ok(bytes) => bytes,
            Err(bits) => return bits,
        };
        let out_ptr = alloc_bytes(_py, &bytes);
        if out_ptr.is_null() {
            raise_exception::<_>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_decode_source(source_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let Some(source_ptr) = obj_from_bits(source_bits).as_ptr() else {
            inc_ref_bits(_py, source_bits);
            return source_bits;
        };
        let source_type = unsafe { object_type_id(source_ptr) };
        if source_type != TYPE_ID_BYTES && source_type != TYPE_ID_BYTEARRAY {
            inc_ref_bits(_py, source_bits);
            return source_bits;
        }
        let encoding_bits = match alloc_str_bits(_py, "utf-8") {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let decoded_bits = if source_type == TYPE_ID_BYTES {
            molt_bytes_decode(source_bits, encoding_bits, MoltObject::none().bits())
        } else {
            molt_bytearray_decode(source_bits, encoding_bits, MoltObject::none().bits())
        };
        dec_ref_bits(_py, encoding_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        decoded_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_source_hash(source_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let source = match bytes_arg_from_bits(_py, source_bits, "source_bytes") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut hasher = Sha1::new();
        hasher.update(&source);
        let digest = hasher.finalize();
        let out_ptr = alloc_bytes(_py, &digest[..8]);
        if out_ptr.is_null() {
            raise_exception::<_>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_source_from_cache(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        inc_ref_bits(_py, path_bits);
        path_bits
    })
}

fn importlib_find_in_path_payload(
    _py: &PyToken<'_>,
    fullname_bits: u64,
    search_paths_bits: u64,
    package_context: bool,
) -> u64 {
    if !has_capability(_py, "fs.read") {
        return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
    }
    let fullname = match string_arg_from_bits(_py, fullname_bits, "module name") {
        Ok(value) => value,
        Err(bits) => return bits,
    };
    let search_paths = match string_sequence_arg_from_bits(_py, search_paths_bits, "search paths") {
        Ok(value) => value,
        Err(bits) => return bits,
    };
    let Some(resolution) = importlib_find_in_path(&fullname, &search_paths, package_context) else {
        return MoltObject::none().bits();
    };
    let origin_bits = match resolution.origin.as_deref() {
        Some(origin) => match alloc_str_bits(_py, origin) {
            Ok(bits) => bits,
            Err(err_bits) => return err_bits,
        },
        None => MoltObject::none().bits(),
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
    let cached_bits = match resolution.cached.as_deref() {
        Some(cached) => match alloc_str_bits(_py, cached) {
            Ok(bits) => bits,
            Err(err_bits) => {
                if !obj_from_bits(origin_bits).is_none() {
                    dec_ref_bits(_py, origin_bits);
                }
                if !obj_from_bits(locations_bits).is_none() {
                    dec_ref_bits(_py, locations_bits);
                }
                return err_bits;
            }
        },
        None => MoltObject::none().bits(),
    };
    let loader_kind_bits = match alloc_str_bits(_py, &resolution.loader_kind) {
        Ok(bits) => bits,
        Err(err_bits) => {
            if !obj_from_bits(origin_bits).is_none() {
                dec_ref_bits(_py, origin_bits);
            }
            if !obj_from_bits(locations_bits).is_none() {
                dec_ref_bits(_py, locations_bits);
            }
            if !obj_from_bits(cached_bits).is_none() {
                dec_ref_bits(_py, cached_bits);
            }
            return err_bits;
        }
    };
    let zip_archive_bits = match resolution.zip_archive.as_deref() {
        Some(path) => match alloc_str_bits(_py, path) {
            Ok(bits) => bits,
            Err(err_bits) => {
                if !obj_from_bits(origin_bits).is_none() {
                    dec_ref_bits(_py, origin_bits);
                }
                if !obj_from_bits(locations_bits).is_none() {
                    dec_ref_bits(_py, locations_bits);
                }
                if !obj_from_bits(cached_bits).is_none() {
                    dec_ref_bits(_py, cached_bits);
                }
                dec_ref_bits(_py, loader_kind_bits);
                return err_bits;
            }
        },
        None => MoltObject::none().bits(),
    };
    let zip_inner_path_bits = match resolution.zip_inner_path.as_deref() {
        Some(path) => match alloc_str_bits(_py, path) {
            Ok(bits) => bits,
            Err(err_bits) => {
                if !obj_from_bits(origin_bits).is_none() {
                    dec_ref_bits(_py, origin_bits);
                }
                if !obj_from_bits(locations_bits).is_none() {
                    dec_ref_bits(_py, locations_bits);
                }
                if !obj_from_bits(cached_bits).is_none() {
                    dec_ref_bits(_py, cached_bits);
                }
                dec_ref_bits(_py, loader_kind_bits);
                if !obj_from_bits(zip_archive_bits).is_none() {
                    dec_ref_bits(_py, zip_archive_bits);
                }
                return err_bits;
            }
        },
        None => MoltObject::none().bits(),
    };
    let is_package_bits = MoltObject::from_bool(resolution.is_package).bits();
    let has_location_bits = MoltObject::from_bool(resolution.has_location).bits();
    let keys_and_values: [(&[u8], u64); 8] = [
        (b"origin", origin_bits),
        (b"is_package", is_package_bits),
        (b"submodule_search_locations", locations_bits),
        (b"cached", cached_bits),
        (b"loader_kind", loader_kind_bits),
        (b"has_location", has_location_bits),
        (b"zip_archive", zip_archive_bits),
        (b"zip_inner_path", zip_inner_path_bits),
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
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resolve_name(name_bits: u64, package_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let name = match string_arg_from_bits(_py, name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if !name.starts_with('.') {
            return match alloc_str_bits(_py, &name) {
                Ok(bits) => bits,
                Err(err) => err,
            };
        }

        let package = match optional_string_arg_from_bits(_py, package_bits, "package") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(package_name) = package.filter(|value| !value.is_empty()) else {
            return raise_exception::<_>(
                _py,
                "TypeError",
                &format!(
                    "the 'package' argument is required to perform a relative import for '{name}'"
                ),
            );
        };

        let level = name
            .as_bytes()
            .iter()
            .take_while(|&&byte| byte == b'.')
            .count();
        if level == 0 {
            return match alloc_str_bits(_py, &name) {
                Ok(bits) => bits,
                Err(err) => err,
            };
        }

        let package_bits: Vec<&str> = package_name.split('.').collect();
        if level > package_bits.len() {
            return raise_exception::<_>(
                _py,
                "ImportError",
                "attempted relative import beyond top-level package",
            );
        }
        let base = package_bits[..(package_bits.len() - level)].join(".");
        let suffix = &name[level..];
        let resolved = if base.is_empty() {
            suffix.to_string()
        } else {
            format!("{base}{suffix}")
        };
        match alloc_str_bits(_py, &resolved) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_known_absent_missing_name(resolved_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let resolved = match string_arg_from_bits(_py, resolved_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(name) = importlib_known_absent_missing_name(_py, &resolved) else {
            return MoltObject::none().bits();
        };
        match alloc_str_bits(_py, &name) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_import_optional(module_name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let name_bits = match alloc_str_bits(_py, &module_name) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let imported_bits = crate::molt_module_import(name_bits);
        if !obj_from_bits(name_bits).is_none() {
            dec_ref_bits(_py, name_bits);
        }
        if !exception_pending(_py) {
            return imported_bits;
        }
        // Optional imports mirror `try: import x; except ImportError: ...`.
        // They should not propagate module absence and simply yield None.
        if clear_pending_if_kind(_py, &["ImportError", "ModuleNotFoundError"]) {
            if !obj_from_bits(imported_bits).is_none() {
                dec_ref_bits(_py, imported_bits);
            }
            return MoltObject::none().bits();
        }
        if !obj_from_bits(imported_bits).is_none() {
            dec_ref_bits(_py, imported_bits);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_import_or_fallback(
    module_name_bits: u64,
    fallback_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let name_bits = match alloc_str_bits(_py, &module_name) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let imported_bits = crate::molt_module_import(name_bits);
        if !obj_from_bits(name_bits).is_none() {
            dec_ref_bits(_py, name_bits);
        }
        if !exception_pending(_py) {
            return imported_bits;
        }
        if clear_pending_if_kind(_py, &["ImportError", "ModuleNotFoundError"]) {
            if !obj_from_bits(imported_bits).is_none() {
                dec_ref_bits(_py, imported_bits);
            }
            inc_ref_bits(_py, fallback_bits);
            return fallback_bits;
        }
        if !obj_from_bits(imported_bits).is_none() {
            dec_ref_bits(_py, imported_bits);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_import_required(module_name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let name_bits = match alloc_str_bits(_py, &module_name) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let imported_bits = crate::molt_module_import(name_bits);
        if !obj_from_bits(name_bits).is_none() {
            dec_ref_bits(_py, name_bits);
        }
        if exception_pending(_py) {
            if !obj_from_bits(imported_bits).is_none() {
                dec_ref_bits(_py, imported_bits);
            }
            return MoltObject::none().bits();
        }
        imported_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_export_attrs(
    module_name_bits: u64,
    export_names_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let export_names =
            match string_sequence_arg_from_bits(_py, export_names_bits, "export names") {
                Ok(values) => values,
                Err(err) => return err,
            };

        let name_bits = match alloc_str_bits(_py, &module_name) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let module_bits = crate::molt_module_import(name_bits);
        if !obj_from_bits(name_bits).is_none() {
            dec_ref_bits(_py, name_bits);
        }
        if exception_pending(_py) {
            if !obj_from_bits(module_bits).is_none() {
                dec_ref_bits(_py, module_bits);
            }
            return MoltObject::none().bits();
        }

        let mut pairs: Vec<u64> = Vec::with_capacity(export_names.len() * 2);
        let mut owned: Vec<u64> = Vec::with_capacity(export_names.len() * 2);
        let missing = missing_bits(_py);
        for export_name in export_names {
            let attr_name_bits = match alloc_str_bits(_py, &export_name) {
                Ok(bits) => bits,
                Err(err) => {
                    for bits in owned {
                        dec_ref_bits(_py, bits);
                    }
                    if !obj_from_bits(module_bits).is_none() {
                        dec_ref_bits(_py, module_bits);
                    }
                    return err;
                }
            };
            let value_bits = molt_getattr_builtin(module_bits, attr_name_bits, missing);
            if !obj_from_bits(attr_name_bits).is_none() {
                dec_ref_bits(_py, attr_name_bits);
            }
            if exception_pending(_py) {
                if !obj_from_bits(value_bits).is_none() {
                    dec_ref_bits(_py, value_bits);
                }
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                if !obj_from_bits(module_bits).is_none() {
                    dec_ref_bits(_py, module_bits);
                }
                return MoltObject::none().bits();
            }
            if is_missing_bits(_py, value_bits) {
                if !obj_from_bits(value_bits).is_none() {
                    dec_ref_bits(_py, value_bits);
                }
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                if !obj_from_bits(module_bits).is_none() {
                    dec_ref_bits(_py, module_bits);
                }
                return raise_exception::<_>(
                    _py,
                    "AttributeError",
                    &format!("module '{module_name}' has no attribute '{export_name}'"),
                );
            }

            let key_bits = match alloc_str_bits(_py, &export_name) {
                Ok(bits) => bits,
                Err(err) => {
                    if !obj_from_bits(value_bits).is_none() {
                        dec_ref_bits(_py, value_bits);
                    }
                    for bits in owned {
                        dec_ref_bits(_py, bits);
                    }
                    if !obj_from_bits(module_bits).is_none() {
                        dec_ref_bits(_py, module_bits);
                    }
                    return err;
                }
            };
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
            owned.push(value_bits);
        }

        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if !obj_from_bits(module_bits).is_none() {
            dec_ref_bits(_py, module_bits);
        }
        if dict_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_load_module_shim(
    bootstrap_bits: u64,
    loader_bits: u64,
    fullname_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        static LOAD_MODULE_SHIM_NAME: AtomicU64 = AtomicU64::new(0);

        let fullname = match string_arg_from_bits(_py, fullname_bits, "fullname") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let call_bits = match importlib_required_callable(
            _py,
            bootstrap_bits,
            &LOAD_MODULE_SHIM_NAME,
            b"_load_module_shim",
            "importlib._bootstrap",
        ) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let fullname_arg_bits = match alloc_str_bits(_py, &fullname) {
            Ok(bits) => bits,
            Err(err) => {
                if !obj_from_bits(call_bits).is_none() {
                    dec_ref_bits(_py, call_bits);
                }
                return err;
            }
        };
        let out = unsafe { call_callable2(_py, call_bits, loader_bits, fullname_arg_bits) };
        if !obj_from_bits(fullname_arg_bits).is_none() {
            dec_ref_bits(_py, fullname_arg_bits);
        }
        if !obj_from_bits(call_bits).is_none() {
            dec_ref_bits(_py, call_bits);
        }
        if exception_pending(_py) {
            if !obj_from_bits(out).is_none() {
                dec_ref_bits(_py, out);
            }
            return MoltObject::none().bits();
        }
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_joinpath(traversable_bits: u64, child_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        static JOINPATH_NAME: AtomicU64 = AtomicU64::new(0);

        let joinpath_name = intern_static_name(_py, &JOINPATH_NAME, b"joinpath");
        let missing = missing_bits(_py);
        let call_bits = molt_getattr_builtin(traversable_bits, joinpath_name, missing);
        if exception_pending(_py) {
            if !obj_from_bits(call_bits).is_none() {
                dec_ref_bits(_py, call_bits);
            }
            return MoltObject::none().bits();
        }
        if is_missing_bits(_py, call_bits) {
            if !obj_from_bits(call_bits).is_none() {
                dec_ref_bits(_py, call_bits);
            }
            return raise_exception::<_>(
                _py,
                "AttributeError",
                "traversable has no attribute 'joinpath'",
            );
        }
        let out = unsafe { call_callable1(_py, call_bits, child_bits) };
        if !obj_from_bits(call_bits).is_none() {
            dec_ref_bits(_py, call_bits);
        }
        if exception_pending(_py) {
            if !obj_from_bits(out).is_none() {
                dec_ref_bits(_py, out);
            }
            return MoltObject::none().bits();
        }
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_open_mode_is_text(mode_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let mode = match string_arg_from_bits(_py, mode_bits, "mode") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if mode == "r" {
            return MoltObject::from_bool(true).bits();
        }
        if mode == "rb" {
            return MoltObject::from_bool(false).bits();
        }
        raise_exception::<_>(
            _py,
            "ValueError",
            &format!("Invalid mode value {mode:?}, only 'r' and 'rb' are supported"),
        )
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_package_leaf_name(package_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let package = match string_arg_from_bits(_py, package_bits, "package") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let leaf = package
            .rsplit_once('.')
            .map(|(_, tail)| tail)
            .unwrap_or(package.as_str());
        let leaf = if leaf.is_empty() {
            package.as_str()
        } else {
            leaf
        };
        match alloc_str_bits(_py, leaf) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_import_module(
    resolved_bits: u64,
    util_bits: u64,
    machinery_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let resolved = match string_arg_from_bits(_py, resolved_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let modules_bits = match importlib_runtime_modules_bits(_py) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let Some(modules_ptr) = obj_from_bits(modules_bits).as_ptr() else {
            if !obj_from_bits(modules_bits).is_none() {
                dec_ref_bits(_py, modules_bits);
            }
            return importlib_modules_runtime_error(_py);
        };
        let resolved_key_bits = match alloc_str_bits(_py, &resolved) {
            Ok(bits) => bits,
            Err(err) => {
                if !obj_from_bits(modules_bits).is_none() {
                    dec_ref_bits(_py, modules_bits);
                }
                return err;
            }
        };
        let out = (|| -> u64 {
            let cached_bits =
                match importlib_dict_get_string_key_bits(_py, modules_ptr, resolved_key_bits) {
                    Ok(bits) => bits,
                    Err(err) => return err,
                };
            if let Some(cached_bits) = cached_bits {
                let is_empty =
                    match importlib_module_is_empty_placeholder(_py, &resolved, cached_bits) {
                        Ok(value) => value,
                        Err(err) => return err,
                    };
                let should_retry =
                    match importlib_module_should_retry_empty(_py, &resolved, cached_bits) {
                        Ok(value) => value,
                        Err(err) => return err,
                    };
                if !is_empty && !should_retry {
                    if let Err(err) =
                        importlib_bind_submodule_on_parent(_py, &resolved, cached_bits, modules_ptr)
                    {
                        return err;
                    }
                    inc_ref_bits(_py, cached_bits);
                    return cached_bits;
                }
                importlib_dict_del_string_key(_py, modules_ptr, resolved_key_bits);
            }

            let imported_bits = match importlib_import_with_fallback(
                _py,
                &resolved,
                resolved_key_bits,
                modules_ptr,
                util_bits,
                machinery_bits,
            ) {
                Ok(bits) => bits,
                Err(err) => {
                    if exception_pending(_py) {
                        importlib_rethrow_pending_exception(_py);
                    }
                    return err;
                }
            };

            let cached_bits =
                match importlib_dict_get_string_key_bits(_py, modules_ptr, resolved_key_bits) {
                    Ok(bits) => bits,
                    Err(err) => {
                        if !obj_from_bits(imported_bits).is_none() {
                            dec_ref_bits(_py, imported_bits);
                        }
                        return err;
                    }
                };
            if let Some(cached_bits) = cached_bits {
                if let Err(err) =
                    importlib_bind_submodule_on_parent(_py, &resolved, cached_bits, modules_ptr)
                {
                    return err;
                }
                inc_ref_bits(_py, cached_bits);
                if cached_bits != imported_bits && !obj_from_bits(imported_bits).is_none() {
                    dec_ref_bits(_py, imported_bits);
                }
                return cached_bits;
            }
            if !obj_from_bits(imported_bits).is_none() {
                if let Err(err) =
                    importlib_bind_submodule_on_parent(_py, &resolved, imported_bits, modules_ptr)
                {
                    if !obj_from_bits(imported_bits).is_none() {
                        dec_ref_bits(_py, imported_bits);
                    }
                    return err;
                }
                return imported_bits;
            }
            raise_exception::<_>(
                _py,
                "ModuleNotFoundError",
                &format!("No module named '{resolved}'"),
            )
        })();
        dec_ref_bits(_py, resolved_key_bits);
        if !obj_from_bits(modules_bits).is_none() {
            dec_ref_bits(_py, modules_bits);
        }
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_frozen_payload(machinery_bits: u64, util_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        static BUILTIN_IMPORTER_NAME: AtomicU64 = AtomicU64::new(0);
        static FROZEN_IMPORTER_NAME: AtomicU64 = AtomicU64::new(0);
        static MODULE_SPEC_NAME: AtomicU64 = AtomicU64::new(0);
        static MODULE_FROM_SPEC_NAME: AtomicU64 = AtomicU64::new(0);
        static SPEC_FROM_LOADER_NAME: AtomicU64 = AtomicU64::new(0);

        let mut owned: Vec<u64> = Vec::new();
        let mut values: Vec<(&[u8], u64)> = Vec::with_capacity(5);

        let builtin_importer_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &BUILTIN_IMPORTER_NAME,
            b"BuiltinImporter",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        owned.push(builtin_importer_bits);
        values.push((b"BuiltinImporter", builtin_importer_bits));

        let frozen_importer_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &FROZEN_IMPORTER_NAME,
            b"FrozenImporter",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(frozen_importer_bits);
        values.push((b"FrozenImporter", frozen_importer_bits));

        let module_spec_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &MODULE_SPEC_NAME,
            b"ModuleSpec",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(module_spec_bits);
        values.push((b"ModuleSpec", module_spec_bits));

        let module_from_spec_bits = match importlib_required_attribute(
            _py,
            util_bits,
            &MODULE_FROM_SPEC_NAME,
            b"module_from_spec",
            "importlib.util",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(module_from_spec_bits);
        values.push((b"module_from_spec", module_from_spec_bits));

        let spec_from_loader_bits = match importlib_required_attribute(
            _py,
            util_bits,
            &SPEC_FROM_LOADER_NAME,
            b"spec_from_loader",
            "importlib.util",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(spec_from_loader_bits);
        values.push((b"spec_from_loader", spec_from_loader_bits));

        let mut pairs: Vec<u64> = Vec::with_capacity(values.len() * 2);
        for (key, value_bits) in values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
        }

        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_typing_private_payload(typing_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        static GENERIC_NAME: AtomicU64 = AtomicU64::new(0);
        static PARAM_SPEC_NAME: AtomicU64 = AtomicU64::new(0);
        static PARAM_SPEC_ARGS_NAME: AtomicU64 = AtomicU64::new(0);
        static PARAM_SPEC_KWARGS_NAME: AtomicU64 = AtomicU64::new(0);
        static TYPE_ALIAS_TYPE_NAME: AtomicU64 = AtomicU64::new(0);
        static TYPE_VAR_NAME: AtomicU64 = AtomicU64::new(0);
        static TYPE_VAR_TUPLE_NAME: AtomicU64 = AtomicU64::new(0);

        let mut owned: Vec<u64> = Vec::new();
        let mut values: Vec<(&[u8], u64)> = Vec::with_capacity(7);

        let generic_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            &GENERIC_NAME,
            b"Generic",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(generic_bits);
        values.push((b"Generic", generic_bits));

        let param_spec_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            &PARAM_SPEC_NAME,
            b"_ParamSpec",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(param_spec_bits);
        values.push((b"ParamSpec", param_spec_bits));

        let param_spec_args_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            &PARAM_SPEC_ARGS_NAME,
            b"_ParamSpecArgs",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(param_spec_args_bits);
        values.push((b"ParamSpecArgs", param_spec_args_bits));

        let param_spec_kwargs_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            &PARAM_SPEC_KWARGS_NAME,
            b"_ParamSpecKwargs",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(param_spec_kwargs_bits);
        values.push((b"ParamSpecKwargs", param_spec_kwargs_bits));

        let type_alias_type_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            &TYPE_ALIAS_TYPE_NAME,
            b"_MoltTypeAlias",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(type_alias_type_bits);
        values.push((b"TypeAliasType", type_alias_type_bits));

        let type_var_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            &TYPE_VAR_NAME,
            b"_TypeVar",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(type_var_bits);
        values.push((b"TypeVar", type_var_bits));

        let type_var_tuple_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            &TYPE_VAR_TUPLE_NAME,
            b"_TypeVarTuple",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(type_var_tuple_bits);
        values.push((b"TypeVarTuple", type_var_tuple_bits));

        let mut pairs: Vec<u64> = Vec::with_capacity(values.len() * 2);
        for (key, value_bits) in values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
        }

        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_metadata_types_payload(
    typing_bits: u64,
    abc_bits: u64,
    contextlib_bits: u64,
    _itertools_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        static ANY_NAME: AtomicU64 = AtomicU64::new(0);
        static DICT_NAME: AtomicU64 = AtomicU64::new(0);
        static ITERATOR_NAME: AtomicU64 = AtomicU64::new(0);
        static LIST_NAME: AtomicU64 = AtomicU64::new(0);
        static META_PATH_FINDER_NAME: AtomicU64 = AtomicU64::new(0);
        static OPTIONAL_NAME: AtomicU64 = AtomicU64::new(0);
        static OVERLOAD_NAME: AtomicU64 = AtomicU64::new(0);
        static PROTOCOL_NAME: AtomicU64 = AtomicU64::new(0);
        static SUPPRESS_NAME: AtomicU64 = AtomicU64::new(0);
        static TYPE_VAR_NAME: AtomicU64 = AtomicU64::new(0);
        static UNION_NAME: AtomicU64 = AtomicU64::new(0);

        let mut owned: Vec<u64> = Vec::new();
        let mut values: Vec<(&[u8], u64)> = Vec::with_capacity(11);

        let any_bits =
            match importlib_required_attribute(_py, typing_bits, &ANY_NAME, b"Any", "typing") {
                Ok(bits) => bits,
                Err(err) => {
                    for bits in owned {
                        dec_ref_bits(_py, bits);
                    }
                    return err;
                }
            };
        owned.push(any_bits);
        values.push((b"Any", any_bits));

        let dict_bits =
            match importlib_required_attribute(_py, typing_bits, &DICT_NAME, b"Dict", "typing") {
                Ok(bits) => bits,
                Err(err) => {
                    for bits in owned {
                        dec_ref_bits(_py, bits);
                    }
                    return err;
                }
            };
        owned.push(dict_bits);
        values.push((b"Dict", dict_bits));

        let iterator_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            &ITERATOR_NAME,
            b"Iterator",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(iterator_bits);
        values.push((b"Iterator", iterator_bits));

        let list_bits =
            match importlib_required_attribute(_py, typing_bits, &LIST_NAME, b"List", "typing") {
                Ok(bits) => bits,
                Err(err) => {
                    for bits in owned {
                        dec_ref_bits(_py, bits);
                    }
                    return err;
                }
            };
        owned.push(list_bits);
        values.push((b"List", list_bits));

        let mapping_bits = dict_bits;
        inc_ref_bits(_py, mapping_bits);
        owned.push(mapping_bits);
        values.push((b"Mapping", mapping_bits));

        let optional_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            &OPTIONAL_NAME,
            b"Optional",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(optional_bits);
        values.push((b"Optional", optional_bits));

        let protocol_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            &PROTOCOL_NAME,
            b"Protocol",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(protocol_bits);
        values.push((b"Protocol", protocol_bits));

        let type_var_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            &TYPE_VAR_NAME,
            b"_TypeVar",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(type_var_bits);
        values.push((b"TypeVar", type_var_bits));

        let union_bits =
            match importlib_required_attribute(_py, typing_bits, &UNION_NAME, b"Union", "typing") {
                Ok(bits) => bits,
                Err(err) => {
                    for bits in owned {
                        dec_ref_bits(_py, bits);
                    }
                    return err;
                }
            };
        owned.push(union_bits);
        values.push((b"Union", union_bits));

        let overload_bits = match importlib_required_attribute(
            _py,
            typing_bits,
            &OVERLOAD_NAME,
            b"overload",
            "typing",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(overload_bits);
        values.push((b"overload", overload_bits));

        let meta_path_finder_bits = match importlib_required_attribute(
            _py,
            abc_bits,
            &META_PATH_FINDER_NAME,
            b"MetaPathFinder",
            "importlib.abc",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(meta_path_finder_bits);
        values.push((b"MetaPathFinder", meta_path_finder_bits));

        let suppress_bits = match importlib_required_attribute(
            _py,
            contextlib_bits,
            &SUPPRESS_NAME,
            b"suppress",
            "contextlib",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(suppress_bits);
        values.push((b"suppress", suppress_bits));

        let mut pairs: Vec<u64> = Vec::with_capacity(values.len() * 2);
        for (key, value_bits) in values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
        }

        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_frozen_external_payload(
    machinery_bits: u64,
    util_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        static BYTECODE_SUFFIXES_NAME: AtomicU64 = AtomicU64::new(0);
        static DEBUG_BYTECODE_SUFFIXES_NAME: AtomicU64 = AtomicU64::new(0);
        static EXTENSION_SUFFIXES_NAME: AtomicU64 = AtomicU64::new(0);
        static MAGIC_NUMBER_NAME: AtomicU64 = AtomicU64::new(0);
        static OPTIMIZED_BYTECODE_SUFFIXES_NAME: AtomicU64 = AtomicU64::new(0);
        static SOURCE_SUFFIXES_NAME: AtomicU64 = AtomicU64::new(0);
        static EXTENSION_FILE_LOADER_NAME: AtomicU64 = AtomicU64::new(0);
        static FILE_FINDER_NAME: AtomicU64 = AtomicU64::new(0);
        static PRIVATE_FILE_LOADER_NAME: AtomicU64 = AtomicU64::new(0);
        static NAMESPACE_LOADER_NAME: AtomicU64 = AtomicU64::new(0);
        static PATH_FINDER_NAME: AtomicU64 = AtomicU64::new(0);
        static SOURCE_FILE_LOADER_NAME: AtomicU64 = AtomicU64::new(0);
        static PRIVATE_SOURCE_LOADER_NAME: AtomicU64 = AtomicU64::new(0);
        static SOURCELESS_FILE_LOADER_NAME: AtomicU64 = AtomicU64::new(0);
        static LOADER_BASICS_NAME: AtomicU64 = AtomicU64::new(0);
        static WINDOWS_REGISTRY_FINDER_NAME: AtomicU64 = AtomicU64::new(0);
        static CACHE_FROM_SOURCE_NAME: AtomicU64 = AtomicU64::new(0);
        static DECODE_SOURCE_NAME: AtomicU64 = AtomicU64::new(0);
        static SOURCE_FROM_CACHE_NAME: AtomicU64 = AtomicU64::new(0);
        static SPEC_FROM_FILE_LOCATION_NAME: AtomicU64 = AtomicU64::new(0);

        let mut owned: Vec<u64> = Vec::new();
        let mut values: Vec<(&[u8], u64)> = Vec::with_capacity(20);

        let bytecode_suffixes_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &BYTECODE_SUFFIXES_NAME,
            b"BYTECODE_SUFFIXES",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(bytecode_suffixes_bits);
        values.push((b"BYTECODE_SUFFIXES", bytecode_suffixes_bits));

        let debug_bytecode_suffixes_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &DEBUG_BYTECODE_SUFFIXES_NAME,
            b"DEBUG_BYTECODE_SUFFIXES",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(debug_bytecode_suffixes_bits);
        values.push((b"DEBUG_BYTECODE_SUFFIXES", debug_bytecode_suffixes_bits));

        let extension_suffixes_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &EXTENSION_SUFFIXES_NAME,
            b"EXTENSION_SUFFIXES",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(extension_suffixes_bits);
        values.push((b"EXTENSION_SUFFIXES", extension_suffixes_bits));

        let magic_number_bits = match importlib_required_attribute(
            _py,
            util_bits,
            &MAGIC_NUMBER_NAME,
            b"MAGIC_NUMBER",
            "importlib.util",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(magic_number_bits);
        values.push((b"MAGIC_NUMBER", magic_number_bits));

        let optimized_bytecode_suffixes_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &OPTIMIZED_BYTECODE_SUFFIXES_NAME,
            b"OPTIMIZED_BYTECODE_SUFFIXES",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(optimized_bytecode_suffixes_bits);
        values.push((
            b"OPTIMIZED_BYTECODE_SUFFIXES",
            optimized_bytecode_suffixes_bits,
        ));

        let source_suffixes_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &SOURCE_SUFFIXES_NAME,
            b"SOURCE_SUFFIXES",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(source_suffixes_bits);
        values.push((b"SOURCE_SUFFIXES", source_suffixes_bits));

        let extension_file_loader_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &EXTENSION_FILE_LOADER_NAME,
            b"ExtensionFileLoader",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(extension_file_loader_bits);
        values.push((b"ExtensionFileLoader", extension_file_loader_bits));

        let file_finder_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &FILE_FINDER_NAME,
            b"FileFinder",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(file_finder_bits);
        values.push((b"FileFinder", file_finder_bits));

        let file_loader_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &PRIVATE_FILE_LOADER_NAME,
            b"_FileLoader",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(file_loader_bits);
        values.push((b"FileLoader", file_loader_bits));

        let namespace_loader_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &NAMESPACE_LOADER_NAME,
            b"NamespaceLoader",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(namespace_loader_bits);
        values.push((b"NamespaceLoader", namespace_loader_bits));

        let path_finder_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &PATH_FINDER_NAME,
            b"PathFinder",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(path_finder_bits);
        values.push((b"PathFinder", path_finder_bits));

        let source_file_loader_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &SOURCE_FILE_LOADER_NAME,
            b"SourceFileLoader",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(source_file_loader_bits);
        values.push((b"SourceFileLoader", source_file_loader_bits));

        let source_loader_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &PRIVATE_SOURCE_LOADER_NAME,
            b"_SourceLoader",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(source_loader_bits);
        values.push((b"SourceLoader", source_loader_bits));

        let sourceless_file_loader_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &SOURCELESS_FILE_LOADER_NAME,
            b"SourcelessFileLoader",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(sourceless_file_loader_bits);
        values.push((b"SourcelessFileLoader", sourceless_file_loader_bits));

        let loader_basics_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &LOADER_BASICS_NAME,
            b"_LoaderBasics",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(loader_basics_bits);
        values.push((b"_LoaderBasics", loader_basics_bits));

        let windows_registry_finder_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &WINDOWS_REGISTRY_FINDER_NAME,
            b"WindowsRegistryFinder",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(windows_registry_finder_bits);
        values.push((b"WindowsRegistryFinder", windows_registry_finder_bits));

        let cache_from_source_bits = match importlib_required_attribute(
            _py,
            util_bits,
            &CACHE_FROM_SOURCE_NAME,
            b"cache_from_source",
            "importlib.util",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(cache_from_source_bits);
        values.push((b"cache_from_source", cache_from_source_bits));

        let decode_source_bits = match importlib_required_attribute(
            _py,
            util_bits,
            &DECODE_SOURCE_NAME,
            b"decode_source",
            "importlib.util",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(decode_source_bits);
        values.push((b"decode_source", decode_source_bits));

        let source_from_cache_bits = match importlib_required_attribute(
            _py,
            util_bits,
            &SOURCE_FROM_CACHE_NAME,
            b"source_from_cache",
            "importlib.util",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(source_from_cache_bits);
        values.push((b"source_from_cache", source_from_cache_bits));

        let spec_from_file_location_bits = match importlib_required_attribute(
            _py,
            util_bits,
            &SPEC_FROM_FILE_LOCATION_NAME,
            b"spec_from_file_location",
            "importlib.util",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return err;
            }
        };
        owned.push(spec_from_file_location_bits);
        values.push((b"spec_from_file_location", spec_from_file_location_bits));

        let mut pairs: Vec<u64> = Vec::with_capacity(values.len() * 2);
        for (key, value_bits) in values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(dict_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_find_in_path(fullname_bits: u64, search_paths_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        importlib_find_in_path_payload(_py, fullname_bits, search_paths_bits, false)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_find_in_path_package_context(
    fullname_bits: u64,
    search_paths_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        importlib_find_in_path_payload(_py, fullname_bits, search_paths_bits, true)
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_find_spec(
    fullname_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
    meta_path_bits: u64,
    path_hooks_bits: u64,
    path_importer_cache_bits: u64,
    package_context_bits: u64,
    machinery_bits: u64,
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
        let package_context = is_truthy(_py, obj_from_bits(package_context_bits));
        match importlib_find_spec_with_runtime_state_bits(
            _py,
            ImportlibRuntimeSpecContext {
                fullname: &fullname,
                search_paths: &search_paths,
                module_file,
                meta_path_bits,
                path_hooks_bits,
                path_importer_cache_bits,
                package_context,
                machinery_bits,
            },
        ) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_find_spec_orchestrate(
    module_name_bits: u64,
    path_snapshot_bits: u64,
    module_file_bits: u64,
    spec_cache_bits: u64,
    machinery_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let path_snapshot =
            match string_sequence_arg_from_bits(_py, path_snapshot_bits, "search paths") {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let module_file = match module_file_from_bits(_py, module_file_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(spec_cache_ptr) = obj_from_bits(spec_cache_bits).as_ptr() else {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "invalid importlib find_spec cache mapping",
            );
        };
        if unsafe { object_type_id(spec_cache_ptr) } != TYPE_ID_DICT {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "invalid importlib find_spec cache mapping",
            );
        }
        match importlib_find_spec_orchestrated_impl(
            _py,
            &module_name,
            &path_snapshot,
            module_file,
            spec_cache_ptr,
            machinery_bits,
        ) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_find_spec_from_path_hooks(
    fullname_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
    path_hooks_bits: u64,
    path_importer_cache_bits: u64,
    package_context_bits: u64,
    machinery_bits: u64,
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
        let package_context = is_truthy(_py, obj_from_bits(package_context_bits));
        importlib_find_spec_from_path_hooks_impl(
            _py,
            ImportlibPathHooksContext {
                fullname: &fullname,
                search_paths: &search_paths,
                module_file,
                path_hooks_bits,
                path_importer_cache_bits,
                package_context,
                machinery_bits,
            },
        )
    })
}

struct ImportlibPathHooksContext<'a> {
    fullname: &'a str,
    search_paths: &'a [String],
    module_file: Option<String>,
    path_hooks_bits: u64,
    path_importer_cache_bits: u64,
    package_context: bool,
    machinery_bits: u64,
}

fn importlib_find_spec_from_path_hooks_impl(
    _py: &PyToken<'_>,
    ctx: ImportlibPathHooksContext<'_>,
) -> u64 {
    let path_hooks_count =
        match iterable_count_arg_from_bits(_py, ctx.path_hooks_bits, "path_hooks") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
    let via_path_hooks = match importlib_find_spec_via_path_hooks(
        _py,
        ctx.fullname,
        ctx.search_paths,
        ctx.path_hooks_bits,
        ctx.path_importer_cache_bits,
    ) {
        Ok(value) => value,
        Err(bits) => return bits,
    };
    if let Some(spec_bits) = via_path_hooks {
        return spec_bits;
    }
    if ctx.fullname != "math" && !has_capability(_py, "fs.read") {
        return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
    }

    // This intrinsic models the PathFinder lane (invoked from meta-path),
    // so meta-path participation is logically non-empty even without a
    // Python sentinel object.
    let meta_path_count = 1;
    let Some(payload) = importlib_find_spec_payload(
        ctx.fullname,
        ctx.search_paths,
        ctx.module_file,
        meta_path_count,
        path_hooks_count,
        ctx.package_context,
    ) else {
        return MoltObject::none().bits();
    };
    match importlib_find_spec_object_bits(_py, ctx.fullname, &payload, ctx.machinery_bits) {
        Ok(bits) => bits,
        Err(err) => err,
    }
}

struct ImportlibRuntimeSpecContext<'a> {
    fullname: &'a str,
    search_paths: &'a [String],
    module_file: Option<String>,
    meta_path_bits: u64,
    path_hooks_bits: u64,
    path_importer_cache_bits: u64,
    package_context: bool,
    machinery_bits: u64,
}

fn importlib_find_spec_with_runtime_state_bits(
    _py: &PyToken<'_>,
    ctx: ImportlibRuntimeSpecContext<'_>,
) -> Result<u64, u64> {
    let meta_path_count = iterable_count_arg_from_bits(_py, ctx.meta_path_bits, "meta_path")?;
    let path_hooks_count = iterable_count_arg_from_bits(_py, ctx.path_hooks_bits, "path_hooks")?;
    let via_meta_path =
        importlib_find_spec_via_meta_path(_py, ctx.fullname, ctx.search_paths, ctx.meta_path_bits)?;
    if let Some(spec_bits) = via_meta_path {
        return Ok(spec_bits);
    }
    // CPython only consults path hooks via meta-path finders (notably PathFinder).
    // If meta_path is empty, find_spec should not probe path_hooks directly.
    let via_path_hooks = if meta_path_count == 0 {
        None
    } else {
        importlib_find_spec_via_path_hooks(
            _py,
            ctx.fullname,
            ctx.search_paths,
            ctx.path_hooks_bits,
            ctx.path_importer_cache_bits,
        )?
    };
    if let Some(spec_bits) = via_path_hooks {
        return Ok(spec_bits);
    }
    if ctx.fullname != "math" && !has_capability(_py, "fs.read") {
        return Err(raise_exception::<_>(
            _py,
            "PermissionError",
            "missing fs.read capability",
        ));
    }
    let Some(payload) = importlib_find_spec_payload(
        ctx.fullname,
        ctx.search_paths,
        ctx.module_file,
        meta_path_count,
        path_hooks_count,
        ctx.package_context,
    ) else {
        return Ok(MoltObject::none().bits());
    };
    importlib_find_spec_object_bits(_py, ctx.fullname, &payload, ctx.machinery_bits)
}

fn importlib_find_spec_orchestrated_search_paths(
    _py: &PyToken<'_>,
    module_name: &str,
    modules_bits: u64,
    path_snapshot: &[String],
    module_file: Option<String>,
    spec_cache_ptr: *mut u8,
    machinery_bits: u64,
) -> Result<(Vec<String>, bool), u64> {
    let parent_payload = importlib_parent_search_paths_payload(_py, module_name, modules_bits)?;
    if !parent_payload.has_parent {
        return Ok((importlib_search_paths(path_snapshot, module_file), false));
    }
    if !parent_payload.needs_parent_spec {
        return Ok((parent_payload.search_paths, parent_payload.package_context));
    }

    let Some(parent_name) = parent_payload.parent_name else {
        return Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            "invalid importlib parent search paths payload: parent_name",
        ));
    };

    static SUBMODULE_SEARCH_LOCATIONS_NAME: AtomicU64 = AtomicU64::new(0);
    let parent_spec_bits = importlib_find_spec_orchestrated_impl(
        _py,
        &parent_name,
        path_snapshot,
        module_file.clone(),
        spec_cache_ptr,
        machinery_bits,
    )?;
    if obj_from_bits(parent_spec_bits).is_none() {
        return Ok((Vec::new(), true));
    }
    let submodule_search_locations_name = intern_static_name(
        _py,
        &SUBMODULE_SEARCH_LOCATIONS_NAME,
        b"submodule_search_locations",
    );
    let parent_paths_bits =
        match getattr_optional_bits(_py, parent_spec_bits, submodule_search_locations_name) {
            Ok(Some(bits)) => bits,
            Ok(None) => MoltObject::none().bits(),
            Err(err) => {
                if !obj_from_bits(parent_spec_bits).is_none() {
                    dec_ref_bits(_py, parent_spec_bits);
                }
                return Err(err);
            }
        };
    if !obj_from_bits(parent_spec_bits).is_none() {
        dec_ref_bits(_py, parent_spec_bits);
    }
    let search_paths = match importlib_coerce_search_paths_values(
        _py,
        parent_paths_bits,
        "invalid parent package search path",
    ) {
        Ok(value) => value,
        Err(err) => {
            if !obj_from_bits(parent_paths_bits).is_none() {
                dec_ref_bits(_py, parent_paths_bits);
            }
            return Err(err);
        }
    };
    if !obj_from_bits(parent_paths_bits).is_none() {
        dec_ref_bits(_py, parent_paths_bits);
    }
    Ok((search_paths, true))
}

fn importlib_find_spec_orchestrated_impl(
    _py: &PyToken<'_>,
    module_name: &str,
    path_snapshot: &[String],
    module_file: Option<String>,
    spec_cache_ptr: *mut u8,
    machinery_bits: u64,
) -> Result<u64, u64> {
    let runtime_state = importlib_runtime_state_view_bits(_py)?;
    let out = (|| -> Result<u64, u64> {
        let existing_spec_bits = importlib_existing_spec_from_modules_bits(
            _py,
            module_name,
            runtime_state.modules_bits,
            machinery_bits,
        )?;
        if !obj_from_bits(existing_spec_bits).is_none() {
            return Ok(existing_spec_bits);
        }

        let (search_paths, package_context) = importlib_find_spec_orchestrated_search_paths(
            _py,
            module_name,
            runtime_state.modules_bits,
            path_snapshot,
            module_file.clone(),
            spec_cache_ptr,
            machinery_bits,
        )?;
        let search_paths_bits = importlib_alloc_string_tuple_bits(_py, &search_paths)?;
        let meta_path_sig_bits = match importlib_finder_signature_tuple_bits(
            _py,
            runtime_state.meta_path_bits,
            "invalid meta_path iterable",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, search_paths_bits);
                return Err(err);
            }
        };
        let path_hooks_sig_bits = match importlib_finder_signature_tuple_bits(
            _py,
            runtime_state.path_hooks_bits,
            "invalid path_hooks iterable",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, search_paths_bits);
                dec_ref_bits(_py, meta_path_sig_bits);
                return Err(err);
            }
        };
        let path_importer_cache_sig_bits = match importlib_path_importer_cache_signature_tuple_bits(
            _py,
            runtime_state.path_importer_cache_bits,
            "invalid path_importer_cache mapping",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, search_paths_bits);
                dec_ref_bits(_py, meta_path_sig_bits);
                dec_ref_bits(_py, path_hooks_sig_bits);
                return Err(err);
            }
        };
        let module_name_bits = match alloc_str_bits(_py, module_name) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, search_paths_bits);
                dec_ref_bits(_py, meta_path_sig_bits);
                dec_ref_bits(_py, path_hooks_sig_bits);
                dec_ref_bits(_py, path_importer_cache_sig_bits);
                return Err(err);
            }
        };
        let cache_key_ptr = alloc_tuple(
            _py,
            &[
                module_name_bits,
                search_paths_bits,
                meta_path_sig_bits,
                path_hooks_sig_bits,
                path_importer_cache_sig_bits,
                MoltObject::from_bool(package_context).bits(),
            ],
        );
        if !obj_from_bits(module_name_bits).is_none() {
            dec_ref_bits(_py, module_name_bits);
        }
        if !obj_from_bits(search_paths_bits).is_none() {
            dec_ref_bits(_py, search_paths_bits);
        }
        if !obj_from_bits(meta_path_sig_bits).is_none() {
            dec_ref_bits(_py, meta_path_sig_bits);
        }
        if !obj_from_bits(path_hooks_sig_bits).is_none() {
            dec_ref_bits(_py, path_hooks_sig_bits);
        }
        if !obj_from_bits(path_importer_cache_sig_bits).is_none() {
            dec_ref_bits(_py, path_importer_cache_sig_bits);
        }
        if cache_key_ptr.is_null() {
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        let cache_key_bits = MoltObject::from_ptr(cache_key_ptr).bits();
        let cached_bits = unsafe { dict_get_in_place(_py, spec_cache_ptr, cache_key_bits) };
        if exception_pending(_py) {
            dec_ref_bits(_py, cache_key_bits);
            return Err(MoltObject::none().bits());
        }
        if let Some(cached_bits) = cached_bits {
            if !obj_from_bits(cached_bits).is_none() {
                inc_ref_bits(_py, cached_bits);
            }
            dec_ref_bits(_py, cache_key_bits);
            return Ok(cached_bits);
        }

        let spec_bits = importlib_find_spec_with_runtime_state_bits(
            _py,
            ImportlibRuntimeSpecContext {
                fullname: module_name,
                search_paths: &search_paths,
                module_file,
                meta_path_bits: runtime_state.meta_path_bits,
                path_hooks_bits: runtime_state.path_hooks_bits,
                path_importer_cache_bits: runtime_state.path_importer_cache_bits,
                package_context,
                machinery_bits,
            },
        )?;
        unsafe {
            dict_set_in_place(_py, spec_cache_ptr, cache_key_bits, spec_bits);
        }
        if exception_pending(_py) {
            if !obj_from_bits(spec_bits).is_none() {
                dec_ref_bits(_py, spec_bits);
            }
            dec_ref_bits(_py, cache_key_bits);
            return Err(MoltObject::none().bits());
        }
        dec_ref_bits(_py, cache_key_bits);
        Ok(spec_bits)
    })();
    for bits in [
        runtime_state.modules_bits,
        runtime_state.meta_path_bits,
        runtime_state.path_hooks_bits,
        runtime_state.path_importer_cache_bits,
    ] {
        if !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
    }
    out
}

fn importlib_sys_module_bits(_py: &PyToken<'_>) -> Option<u64> {
    let module_cache = crate::builtins::exceptions::internals::module_cache(_py);
    let guard = module_cache.lock().unwrap();
    guard.get("sys").copied()
}

fn importlib_machinery_module_file(
    _py: &PyToken<'_>,
    machinery_bits: u64,
) -> Result<Option<String>, u64> {
    static FILE_NAME: AtomicU64 = AtomicU64::new(0);
    let file_name = intern_static_name(_py, &FILE_NAME, b"__file__");
    let file_bits = getattr_optional_bits(_py, machinery_bits, file_name)?;
    match file_bits {
        Some(bits) => {
            let out = match module_file_from_bits(_py, bits) {
                Ok(value) => value,
                Err(err) => {
                    if !obj_from_bits(bits).is_none() {
                        dec_ref_bits(_py, bits);
                    }
                    return Err(err);
                }
            };
            if !obj_from_bits(bits).is_none() {
                dec_ref_bits(_py, bits);
            }
            Ok(out)
        }
        None => Ok(None),
    }
}

fn importlib_runtime_path_hooks_and_cache_bits(
    _py: &PyToken<'_>,
    sys_bits: Option<u64>,
) -> Result<(u64, bool, u64, bool), u64> {
    static PATH_HOOKS_NAME: AtomicU64 = AtomicU64::new(0);
    static PATH_IMPORTER_CACHE_NAME: AtomicU64 = AtomicU64::new(0);

    let mut path_hooks_bits = MoltObject::none().bits();
    let mut owns_path_hooks = false;
    let mut path_importer_cache_bits = MoltObject::none().bits();
    let mut owns_path_importer_cache = false;

    if let Some(sys_bits) = sys_bits
        && !obj_from_bits(sys_bits).is_none()
    {
        let path_hooks_name = intern_static_name(_py, &PATH_HOOKS_NAME, b"path_hooks");
        let hooks_attr = getattr_optional_bits(_py, sys_bits, path_hooks_name)?;
        if let Some(bits) = hooks_attr {
            path_hooks_bits = bits;
            owns_path_hooks = true;
        } else {
            let empty_hooks_ptr = alloc_tuple(_py, &[]);
            if empty_hooks_ptr.is_null() {
                return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
            }
            path_hooks_bits = MoltObject::from_ptr(empty_hooks_ptr).bits();
            owns_path_hooks = true;
        }

        let path_importer_cache_name =
            intern_static_name(_py, &PATH_IMPORTER_CACHE_NAME, b"path_importer_cache");
        let cache_attr = getattr_optional_bits(_py, sys_bits, path_importer_cache_name)?;
        if let Some(bits) = cache_attr {
            path_importer_cache_bits = bits;
            owns_path_importer_cache = true;
        }
    }

    if !owns_path_hooks {
        let empty_hooks_ptr = alloc_tuple(_py, &[]);
        if empty_hooks_ptr.is_null() {
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
        path_hooks_bits = MoltObject::from_ptr(empty_hooks_ptr).bits();
        owns_path_hooks = true;
    }

    Ok((
        path_hooks_bits,
        owns_path_hooks,
        path_importer_cache_bits,
        owns_path_importer_cache,
    ))
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_pathfinder_find_spec(
    fullname_bits: u64,
    path_bits: u64,
    machinery_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        static PATH_NAME: AtomicU64 = AtomicU64::new(0);

        let fullname = match string_arg_from_bits(_py, fullname_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let sys_bits = importlib_sys_module_bits(_py);
        let module_file = match importlib_machinery_module_file(_py, machinery_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };

        let package_context = !obj_from_bits(path_bits).is_none();
        let search_paths: Vec<String> = if package_context {
            match importlib_coerce_search_paths_values(
                _py,
                path_bits,
                "invalid parent package search path",
            ) {
                Ok(value) => value,
                Err(bits) => return bits,
            }
        } else if let Some(sys_bits) = sys_bits {
            if obj_from_bits(sys_bits).is_none() {
                Vec::new()
            } else {
                let path_name = intern_static_name(_py, &PATH_NAME, b"path");
                let path_attr = match getattr_optional_bits(_py, sys_bits, path_name) {
                    Ok(value) => value,
                    Err(bits) => return bits,
                };
                match path_attr {
                    Some(bits) => {
                        let out = match string_sequence_arg_from_bits(_py, bits, "search paths") {
                            Ok(value) => value,
                            Err(err) => {
                                if !obj_from_bits(bits).is_none() {
                                    dec_ref_bits(_py, bits);
                                }
                                return err;
                            }
                        };
                        if !obj_from_bits(bits).is_none() {
                            dec_ref_bits(_py, bits);
                        }
                        out
                    }
                    None => Vec::new(),
                }
            }
        } else {
            Vec::new()
        };

        let (path_hooks_bits, owns_path_hooks, path_importer_cache_bits, owns_path_importer_cache) =
            match importlib_runtime_path_hooks_and_cache_bits(_py, sys_bits) {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let result = importlib_find_spec_from_path_hooks_impl(
            _py,
            ImportlibPathHooksContext {
                fullname: &fullname,
                search_paths: &search_paths,
                module_file,
                path_hooks_bits,
                path_importer_cache_bits,
                package_context,
                machinery_bits,
            },
        );
        if owns_path_hooks && !obj_from_bits(path_hooks_bits).is_none() {
            dec_ref_bits(_py, path_hooks_bits);
        }
        if owns_path_importer_cache && !obj_from_bits(path_importer_cache_bits).is_none() {
            dec_ref_bits(_py, path_importer_cache_bits);
        }
        result
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_filefinder_find_spec(
    fullname_bits: u64,
    path_bits: u64,
    machinery_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let fullname = match string_arg_from_bits(_py, fullname_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let sys_bits = importlib_sys_module_bits(_py);
        let module_file = match importlib_machinery_module_file(_py, machinery_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let (path_hooks_bits, owns_path_hooks, path_importer_cache_bits, owns_path_importer_cache) =
            match importlib_runtime_path_hooks_and_cache_bits(_py, sys_bits) {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let search_paths = vec![path];
        let result = importlib_find_spec_from_path_hooks_impl(
            _py,
            ImportlibPathHooksContext {
                fullname: &fullname,
                search_paths: &search_paths,
                module_file,
                path_hooks_bits,
                path_importer_cache_bits,
                package_context: true,
                machinery_bits,
            },
        );
        if owns_path_hooks && !obj_from_bits(path_hooks_bits).is_none() {
            dec_ref_bits(_py, path_hooks_bits);
        }
        if owns_path_importer_cache && !obj_from_bits(path_importer_cache_bits).is_none() {
            dec_ref_bits(_py, path_importer_cache_bits);
        }
        result
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_invalidate_caches() -> u64 {
    crate::with_gil_entry!(_py, {
        static SPEC_CACHE_NAME: AtomicU64 = AtomicU64::new(0);
        static PATH_IMPORTER_CACHE_NAME: AtomicU64 = AtomicU64::new(0);

        if let Some(util_bits) = importlib_module_cache_lookup_bits(_py, "importlib.util")
            && !obj_from_bits(util_bits).is_none()
        {
            importlib_clear_mapping_attr_best_effort(
                _py,
                util_bits,
                &SPEC_CACHE_NAME,
                b"_SPEC_CACHE",
            );
        }
        if let Some(sys_bits) = importlib_module_cache_lookup_bits(_py, "sys")
            && !obj_from_bits(sys_bits).is_none()
        {
            importlib_clear_mapping_attr_best_effort(
                _py,
                sys_bits,
                &PATH_IMPORTER_CACHE_NAME,
                b"path_importer_cache",
            );
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_filefinder_invalidate(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        static PATH_IMPORTER_CACHE_NAME: AtomicU64 = AtomicU64::new(0);
        static POP_NAME: AtomicU64 = AtomicU64::new(0);

        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(sys_bits) = importlib_module_cache_lookup_bits(_py, "sys") else {
            return MoltObject::none().bits();
        };
        if obj_from_bits(sys_bits).is_none() {
            return MoltObject::none().bits();
        }
        let path_importer_cache_name =
            intern_static_name(_py, &PATH_IMPORTER_CACHE_NAME, b"path_importer_cache");
        let path_importer_cache_bits =
            match getattr_optional_bits(_py, sys_bits, path_importer_cache_name) {
                Ok(Some(bits)) => bits,
                Ok(None) => return MoltObject::none().bits(),
                Err(_) => {
                    if exception_pending(_py) {
                        clear_exception(_py);
                    }
                    return MoltObject::none().bits();
                }
            };

        if let Some(path_importer_cache_ptr) = obj_from_bits(path_importer_cache_bits).as_ptr() {
            if unsafe { object_type_id(path_importer_cache_ptr) } == TYPE_ID_DICT {
                let path_key_bits = match alloc_str_bits(_py, &path) {
                    Ok(bits) => bits,
                    Err(err) => {
                        if !obj_from_bits(path_importer_cache_bits).is_none() {
                            dec_ref_bits(_py, path_importer_cache_bits);
                        }
                        return err;
                    }
                };
                importlib_dict_del_string_key(_py, path_importer_cache_ptr, path_key_bits);
                if !obj_from_bits(path_key_bits).is_none() {
                    dec_ref_bits(_py, path_key_bits);
                }
            } else {
                let pop_result = match importlib_lookup_callable_attr(
                    _py,
                    path_importer_cache_bits,
                    &POP_NAME,
                    b"pop",
                ) {
                    Ok(Some(pop_bits)) => {
                        let path_key_bits = match alloc_str_bits(_py, &path) {
                            Ok(bits) => bits,
                            Err(err) => {
                                if !obj_from_bits(pop_bits).is_none() {
                                    dec_ref_bits(_py, pop_bits);
                                }
                                if !obj_from_bits(path_importer_cache_bits).is_none() {
                                    dec_ref_bits(_py, path_importer_cache_bits);
                                }
                                return err;
                            }
                        };
                        let out = call_callable_positional(
                            _py,
                            pop_bits,
                            &[path_key_bits, MoltObject::none().bits()],
                        );
                        if !obj_from_bits(pop_bits).is_none() {
                            dec_ref_bits(_py, pop_bits);
                        }
                        if !obj_from_bits(path_key_bits).is_none() {
                            dec_ref_bits(_py, path_key_bits);
                        }
                        out
                    }
                    Ok(None) => Ok(MoltObject::none().bits()),
                    Err(_) => {
                        if exception_pending(_py) {
                            clear_exception(_py);
                        }
                        Ok(MoltObject::none().bits())
                    }
                };
                if let Ok(result_bits) = pop_result
                    && !obj_from_bits(result_bits).is_none()
                {
                    dec_ref_bits(_py, result_bits);
                }
                if exception_pending(_py) {
                    clear_exception(_py);
                }
            }
        }
        if !obj_from_bits(path_importer_cache_bits).is_none() {
            dec_ref_bits(_py, path_importer_cache_bits);
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_reload(
    module_bits: u64,
    util_bits: u64,
    machinery_bits: u64,
    import_module_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        static NAME_NAME: AtomicU64 = AtomicU64::new(0);
        static FILE_NAME: AtomicU64 = AtomicU64::new(0);
        static SPEC_NAME: AtomicU64 = AtomicU64::new(0);
        static LOADER_NAME: AtomicU64 = AtomicU64::new(0);
        static PATH_NAME: AtomicU64 = AtomicU64::new(0);
        static SPEC_FROM_FILE_LOCATION_NAME: AtomicU64 = AtomicU64::new(0);
        static FIND_SPEC_NAME: AtomicU64 = AtomicU64::new(0);
        static EXEC_MODULE_NAME: AtomicU64 = AtomicU64::new(0);
        static LOAD_MODULE_NAME: AtomicU64 = AtomicU64::new(0);

        let mut name_bits = MoltObject::none().bits();
        let mut module_loader_bits = MoltObject::none().bits();
        let mut module_loader_owned = false;
        let mut modules_bits = MoltObject::none().bits();

        let out = (|| -> Result<u64, u64> {
            let module_name_name = intern_static_name(_py, &NAME_NAME, b"__name__");
            let spec_name = intern_static_name(_py, &SPEC_NAME, b"__spec__");
            let mut module_name_bits =
                if let Some(spec_bits) = getattr_optional_bits(_py, module_bits, spec_name)? {
                    let out = getattr_optional_bits(_py, spec_bits, module_name_name)?;
                    if !obj_from_bits(spec_bits).is_none() {
                        dec_ref_bits(_py, spec_bits);
                    }
                    out
                } else {
                    None
                };
            if module_name_bits.is_none() {
                module_name_bits = getattr_optional_bits(_py, module_bits, module_name_name)?;
            }
            let Some(module_name_bits) = module_name_bits else {
                return Err(raise_exception::<_>(
                    _py,
                    "TypeError",
                    "reload() argument must be a module",
                ));
            };

            modules_bits = importlib_runtime_modules_bits(_py)?;
            let Some(modules_ptr) = obj_from_bits(modules_bits).as_ptr() else {
                if !obj_from_bits(module_name_bits).is_none() {
                    dec_ref_bits(_py, module_name_bits);
                }
                return Err(importlib_modules_runtime_error(_py));
            };
            let in_sys_modules = unsafe { dict_get_in_place(_py, modules_ptr, module_name_bits) }
                .is_some_and(|bits| bits == module_bits);
            if exception_pending(_py) {
                if !obj_from_bits(module_name_bits).is_none() {
                    dec_ref_bits(_py, module_name_bits);
                }
                return Err(MoltObject::none().bits());
            }
            if !in_sys_modules {
                let display = format_obj_str(_py, obj_from_bits(module_name_bits));
                if !obj_from_bits(module_name_bits).is_none() {
                    dec_ref_bits(_py, module_name_bits);
                }
                return Err(raise_exception::<_>(
                    _py,
                    "ImportError",
                    &format!("module {display} not in sys.modules"),
                ));
            }

            let module_name_obj = obj_from_bits(module_name_bits);
            let Some(module_name) = string_obj_to_owned(module_name_obj) else {
                let module_name_type = type_name(_py, module_name_obj);
                if !module_name_obj.is_none() {
                    dec_ref_bits(_py, module_name_bits);
                }
                return Err(raise_exception::<_>(
                    _py,
                    "AttributeError",
                    &format!("'{module_name_type}' object has no attribute 'rpartition'"),
                ));
            };
            if !module_name_obj.is_none() {
                dec_ref_bits(_py, module_name_bits);
            }
            name_bits = alloc_str_bits(_py, &module_name)?;

            let module_file_name = intern_static_name(_py, &FILE_NAME, b"__file__");
            let module_file = match getattr_optional_bits(_py, module_bits, module_file_name)? {
                Some(bits) => {
                    let out = string_obj_to_owned(obj_from_bits(bits));
                    if !obj_from_bits(bits).is_none() {
                        dec_ref_bits(_py, bits);
                    }
                    out
                }
                None => None,
            };

            let loader_name = intern_static_name(_py, &LOADER_NAME, b"loader");
            if let Some(spec_bits) = getattr_optional_bits(_py, module_bits, spec_name)? {
                if !obj_from_bits(spec_bits).is_none()
                    && let Some(loader_bits) = getattr_optional_bits(_py, spec_bits, loader_name)?
                    && !obj_from_bits(loader_bits).is_none()
                {
                    module_loader_bits = loader_bits;
                    module_loader_owned = true;
                }
                if !obj_from_bits(spec_bits).is_none() {
                    dec_ref_bits(_py, spec_bits);
                }
            }

            if let Some(module_file) = module_file {
                let module_file_bits = alloc_str_bits(_py, &module_file)?;
                let mut locations_bits = MoltObject::none().bits();
                let mut locations_owned = false;
                let path_name = intern_static_name(_py, &PATH_NAME, b"__path__");
                if let Some(path_bits) = getattr_optional_bits(_py, module_bits, path_name)? {
                    if let Some(path_ptr) = obj_from_bits(path_bits).as_ptr() {
                        let type_id = unsafe { object_type_id(path_ptr) };
                        if type_id == TYPE_ID_LIST || type_id == TYPE_ID_TUPLE {
                            locations_bits = importlib_list_from_iterable(
                                _py,
                                path_bits,
                                "submodule_search_locations",
                            )?;
                            locations_owned = true;
                        }
                    }
                    if !obj_from_bits(path_bits).is_none() {
                        dec_ref_bits(_py, path_bits);
                    }
                }

                let mut loader_override_bits = module_loader_bits;
                if !obj_from_bits(loader_override_bits).is_none()
                    && importlib_loader_is_molt_loader(_py, loader_override_bits, machinery_bits)?
                {
                    loader_override_bits = MoltObject::none().bits();
                }

                let spec_from_file_location_bits = importlib_required_callable(
                    _py,
                    util_bits,
                    &SPEC_FROM_FILE_LOCATION_NAME,
                    b"spec_from_file_location",
                    "importlib.util",
                )?;
                let spec_bits = call_callable_positional(
                    _py,
                    spec_from_file_location_bits,
                    &[
                        name_bits,
                        module_file_bits,
                        loader_override_bits,
                        locations_bits,
                    ],
                )?;
                if !obj_from_bits(spec_from_file_location_bits).is_none() {
                    dec_ref_bits(_py, spec_from_file_location_bits);
                }
                if !obj_from_bits(module_file_bits).is_none() {
                    dec_ref_bits(_py, module_file_bits);
                }
                if locations_owned && !obj_from_bits(locations_bits).is_none() {
                    dec_ref_bits(_py, locations_bits);
                }

                if !obj_from_bits(spec_bits).is_none() {
                    let mut loaded = false;
                    if let Some(spec_loader_bits) =
                        getattr_optional_bits(_py, spec_bits, loader_name)?
                        && !obj_from_bits(spec_loader_bits).is_none()
                    {
                        if let Some(exec_bits) = importlib_lookup_callable_attr(
                            _py,
                            spec_loader_bits,
                            &EXEC_MODULE_NAME,
                            b"exec_module",
                        )? {
                            let exec_out_bits =
                                call_callable_positional(_py, exec_bits, &[module_bits])?;
                            if !obj_from_bits(exec_bits).is_none() {
                                dec_ref_bits(_py, exec_bits);
                            }
                            if !obj_from_bits(exec_out_bits).is_none() {
                                dec_ref_bits(_py, exec_out_bits);
                            }
                            importlib_dict_set_string_key(
                                _py,
                                modules_ptr,
                                name_bits,
                                module_bits,
                            )?;
                            inc_ref_bits(_py, module_bits);
                            loaded = true;
                        }
                        if !obj_from_bits(spec_loader_bits).is_none() {
                            dec_ref_bits(_py, spec_loader_bits);
                        }
                    }
                    if !obj_from_bits(spec_bits).is_none() {
                        dec_ref_bits(_py, spec_bits);
                    }
                    if loaded {
                        return Ok(module_bits);
                    }
                }
            }

            if !obj_from_bits(module_loader_bits).is_none()
                && let Some(exec_bits) = importlib_lookup_callable_attr(
                    _py,
                    module_loader_bits,
                    &EXEC_MODULE_NAME,
                    b"exec_module",
                )?
            {
                let exec_out_bits = call_callable_positional(_py, exec_bits, &[module_bits])?;
                if !obj_from_bits(exec_bits).is_none() {
                    dec_ref_bits(_py, exec_bits);
                }
                if !obj_from_bits(exec_out_bits).is_none() {
                    dec_ref_bits(_py, exec_out_bits);
                }
                importlib_dict_set_string_key(_py, modules_ptr, name_bits, module_bits)?;
                inc_ref_bits(_py, module_bits);
                return Ok(module_bits);
            }

            let find_spec_bits = importlib_required_callable(
                _py,
                util_bits,
                &FIND_SPEC_NAME,
                b"find_spec",
                "importlib.util",
            )?;
            let spec_bits = call_callable_positional(
                _py,
                find_spec_bits,
                &[name_bits, MoltObject::none().bits()],
            )?;
            if !obj_from_bits(find_spec_bits).is_none() {
                dec_ref_bits(_py, find_spec_bits);
            }
            if !obj_from_bits(spec_bits).is_none() {
                if let Some(spec_loader_bits) = getattr_optional_bits(_py, spec_bits, loader_name)?
                    && !obj_from_bits(spec_loader_bits).is_none()
                {
                    if let Some(exec_bits) = importlib_lookup_callable_attr(
                        _py,
                        spec_loader_bits,
                        &EXEC_MODULE_NAME,
                        b"exec_module",
                    )? {
                        let exec_out_bits =
                            call_callable_positional(_py, exec_bits, &[module_bits])?;
                        if !obj_from_bits(exec_bits).is_none() {
                            dec_ref_bits(_py, exec_bits);
                        }
                        if !obj_from_bits(exec_out_bits).is_none() {
                            dec_ref_bits(_py, exec_out_bits);
                        }
                        if !obj_from_bits(spec_loader_bits).is_none() {
                            dec_ref_bits(_py, spec_loader_bits);
                        }
                        if !obj_from_bits(spec_bits).is_none() {
                            dec_ref_bits(_py, spec_bits);
                        }
                        inc_ref_bits(_py, module_bits);
                        return Ok(module_bits);
                    }
                    if let Some(load_bits) = importlib_lookup_callable_attr(
                        _py,
                        spec_loader_bits,
                        &LOAD_MODULE_NAME,
                        b"load_module",
                    )? {
                        let loaded_bits = call_callable_positional(_py, load_bits, &[name_bits])?;
                        if !obj_from_bits(load_bits).is_none() {
                            dec_ref_bits(_py, load_bits);
                        }
                        if !obj_from_bits(spec_loader_bits).is_none() {
                            dec_ref_bits(_py, spec_loader_bits);
                        }
                        if !obj_from_bits(spec_bits).is_none() {
                            dec_ref_bits(_py, spec_bits);
                        }
                        return Ok(loaded_bits);
                    }
                    if !obj_from_bits(spec_loader_bits).is_none() {
                        dec_ref_bits(_py, spec_loader_bits);
                    }
                }
                if !obj_from_bits(spec_bits).is_none() {
                    dec_ref_bits(_py, spec_bits);
                }
            }

            importlib_dict_del_string_key(_py, modules_ptr, name_bits);
            call_callable_positional(_py, import_module_bits, &[name_bits])
        })();

        if module_loader_owned && !obj_from_bits(module_loader_bits).is_none() {
            dec_ref_bits(_py, module_loader_bits);
        }
        if !obj_from_bits(name_bits).is_none() {
            dec_ref_bits(_py, name_bits);
        }
        if !obj_from_bits(modules_bits).is_none() {
            dec_ref_bits(_py, modules_bits);
        }
        match out {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_find_spec_payload(
    fullname_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
    meta_path_bits: u64,
    path_hooks_bits: u64,
    path_importer_cache_bits: u64,
    package_context_bits: u64,
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
        let package_context = is_truthy(_py, obj_from_bits(package_context_bits));
        let via_meta_path = match importlib_find_spec_via_meta_path(
            _py,
            &fullname,
            &search_paths,
            meta_path_bits,
        ) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if let Some(spec_bits) = via_meta_path {
            return match importlib_find_spec_direct_payload_bits(
                _py,
                spec_bits,
                meta_path_count,
                path_hooks_count,
            ) {
                Ok(bits) => bits,
                Err(err) => err,
            };
        }
        // CPython only consults path hooks via meta-path finders (notably PathFinder).
        // If meta_path is empty, find_spec should not probe path_hooks directly.
        let via_path_hooks = if meta_path_count == 0 {
            None
        } else {
            match importlib_find_spec_via_path_hooks(
                _py,
                &fullname,
                &search_paths,
                path_hooks_bits,
                path_importer_cache_bits,
            ) {
                Ok(value) => value,
                Err(bits) => return bits,
            }
        };
        if let Some(spec_bits) = via_path_hooks {
            return match importlib_find_spec_direct_payload_bits(
                _py,
                spec_bits,
                meta_path_count,
                path_hooks_count,
            ) {
                Ok(bits) => bits,
                Err(err) => err,
            };
        }
        if fullname != "math" && !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let Some(payload) = importlib_find_spec_payload(
            &fullname,
            &search_paths,
            module_file,
            meta_path_count,
            path_hooks_count,
            package_context,
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
        let zip_archive_bits = match payload.zip_archive.as_deref() {
            Some(path) => match alloc_str_bits(_py, path) {
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
                    dec_ref_bits(_py, loader_kind_bits);
                    return err;
                }
            },
            None => MoltObject::none().bits(),
        };
        let zip_inner_path_bits = match payload.zip_inner_path.as_deref() {
            Some(path) => match alloc_str_bits(_py, path) {
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
                    dec_ref_bits(_py, loader_kind_bits);
                    if !obj_from_bits(zip_archive_bits).is_none() {
                        dec_ref_bits(_py, zip_archive_bits);
                    }
                    return err;
                }
            },
            None => MoltObject::none().bits(),
        };
        let is_package_bits = MoltObject::from_bool(payload.is_package).bits();
        let is_builtin_bits = MoltObject::from_bool(payload.is_builtin).bits();
        let has_location_bits = MoltObject::from_bool(payload.has_location).bits();
        let meta_path_count_bits = int_bits_from_i64(_py, payload.meta_path_count);
        let path_hooks_count_bits = int_bits_from_i64(_py, payload.path_hooks_count);
        let keys_and_values: [(&[u8], u64); 11] = [
            (b"origin", origin_bits),
            (b"is_package", is_package_bits),
            (b"submodule_search_locations", locations_bits),
            (b"cached", cached_bits),
            (b"is_builtin", is_builtin_bits),
            (b"has_location", has_location_bits),
            (b"loader_kind", loader_kind_bits),
            (b"zip_archive", zip_archive_bits),
            (b"zip_inner_path", zip_inner_path_bits),
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_runtime_state_payload() -> u64 {
    crate::with_gil_entry!(_py, {
        match importlib_runtime_state_payload_bits(_py) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_runtime_modules() -> u64 {
    crate::with_gil_entry!(_py, {
        match importlib_runtime_modules_bits(_py) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_runtime_state_view() -> u64 {
    crate::with_gil_entry!(_py, {
        match importlib_runtime_state_payload_bits(_py) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_existing_spec(
    module_name_bits: u64,
    modules_bits: u64,
    machinery_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        static SPEC_NAME: AtomicU64 = AtomicU64::new(0);
        static FILE_NAME: AtomicU64 = AtomicU64::new(0);
        static MODULE_SPEC_NAME: AtomicU64 = AtomicU64::new(0);

        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };

        let Some(modules_ptr) = obj_from_bits(modules_bits).as_ptr() else {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "invalid importlib runtime state payload: modules",
            );
        };
        if unsafe { object_type_id(modules_ptr) } != TYPE_ID_DICT {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "invalid importlib runtime state payload: modules",
            );
        }

        let module_name_key_bits = match alloc_str_bits(_py, &module_name) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let existing_bits =
            match importlib_dict_get_string_key_bits(_py, modules_ptr, module_name_key_bits) {
                Ok(value) => value,
                Err(err) => {
                    if !obj_from_bits(module_name_key_bits).is_none() {
                        dec_ref_bits(_py, module_name_key_bits);
                    }
                    return err;
                }
            };
        let Some(existing_bits) = existing_bits else {
            if !obj_from_bits(module_name_key_bits).is_none() {
                dec_ref_bits(_py, module_name_key_bits);
            }
            return MoltObject::none().bits();
        };

        let spec_name = intern_static_name(_py, &SPEC_NAME, b"__spec__");
        if let Some(spec_bits) = match getattr_optional_bits(_py, existing_bits, spec_name) {
            Ok(value) => value,
            Err(err) => {
                if !obj_from_bits(module_name_key_bits).is_none() {
                    dec_ref_bits(_py, module_name_key_bits);
                }
                return err;
            }
        } && !obj_from_bits(spec_bits).is_none()
        {
            if !obj_from_bits(module_name_key_bits).is_none() {
                dec_ref_bits(_py, module_name_key_bits);
            }
            return spec_bits;
        }

        let file_name = intern_static_name(_py, &FILE_NAME, b"__file__");
        let origin_bits = match getattr_optional_bits(_py, existing_bits, file_name) {
            Ok(Some(bits)) => {
                if string_obj_to_owned(obj_from_bits(bits)).is_some() {
                    bits
                } else {
                    if !obj_from_bits(bits).is_none() {
                        dec_ref_bits(_py, bits);
                    }
                    MoltObject::none().bits()
                }
            }
            Ok(None) => MoltObject::none().bits(),
            Err(err) => {
                if !obj_from_bits(module_name_key_bits).is_none() {
                    dec_ref_bits(_py, module_name_key_bits);
                }
                return err;
            }
        };
        let module_spec_cls_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &MODULE_SPEC_NAME,
            b"ModuleSpec",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => {
                if !obj_from_bits(origin_bits).is_none() {
                    dec_ref_bits(_py, origin_bits);
                }
                if !obj_from_bits(module_name_key_bits).is_none() {
                    dec_ref_bits(_py, module_name_key_bits);
                }
                return err;
            }
        };
        let is_package_bits = MoltObject::from_bool(false).bits();
        let out = match call_callable_positional(
            _py,
            module_spec_cls_bits,
            &[
                module_name_key_bits,
                MoltObject::none().bits(),
                origin_bits,
                is_package_bits,
            ],
        ) {
            Ok(bits) => bits,
            Err(err) => err,
        };
        if !obj_from_bits(module_spec_cls_bits).is_none() {
            dec_ref_bits(_py, module_spec_cls_bits);
        }
        if !obj_from_bits(origin_bits).is_none() {
            dec_ref_bits(_py, origin_bits);
        }
        if !obj_from_bits(module_name_key_bits).is_none() {
            dec_ref_bits(_py, module_name_key_bits);
        }
        out
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_parent_search_paths(
    module_name_bits: u64,
    modules_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        static DUNDER_PATH_NAME: AtomicU64 = AtomicU64::new(0);

        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let Some(modules_ptr) = obj_from_bits(modules_bits).as_ptr() else {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "invalid importlib runtime state payload: modules",
            );
        };
        if unsafe { object_type_id(modules_ptr) } != TYPE_ID_DICT {
            return raise_exception::<_>(
                _py,
                "RuntimeError",
                "invalid importlib runtime state payload: modules",
            );
        }

        let parent_name = module_name
            .rsplit_once('.')
            .and_then(|(parent, _)| (!parent.is_empty()).then_some(parent.to_string()));

        let (has_parent, needs_parent_spec, package_context, parent_name_bits, search_paths_bits) =
            if let Some(parent_name) = parent_name {
                let parent_key_bits = match alloc_str_bits(_py, &parent_name) {
                    Ok(bits) => bits,
                    Err(err) => return err,
                };
                let parent_bits =
                    match importlib_dict_get_string_key_bits(_py, modules_ptr, parent_key_bits) {
                        Ok(value) => value,
                        Err(err) => {
                            if !obj_from_bits(parent_key_bits).is_none() {
                                dec_ref_bits(_py, parent_key_bits);
                            }
                            return err;
                        }
                    };
                if let Some(parent_bits) = parent_bits {
                    let path_name = intern_static_name(_py, &DUNDER_PATH_NAME, b"__path__");
                    let parent_path_bits = match getattr_optional_bits(_py, parent_bits, path_name)
                    {
                        Ok(Some(bits)) => bits,
                        Ok(None) => MoltObject::none().bits(),
                        Err(err) => {
                            if !obj_from_bits(parent_key_bits).is_none() {
                                dec_ref_bits(_py, parent_key_bits);
                            }
                            return err;
                        }
                    };
                    let search_paths = match importlib_coerce_search_paths_values(
                        _py,
                        parent_path_bits,
                        "invalid parent package search path",
                    ) {
                        Ok(value) => value,
                        Err(err) => {
                            if !obj_from_bits(parent_path_bits).is_none() {
                                dec_ref_bits(_py, parent_path_bits);
                            }
                            if !obj_from_bits(parent_key_bits).is_none() {
                                dec_ref_bits(_py, parent_key_bits);
                            }
                            return err;
                        }
                    };
                    if !obj_from_bits(parent_path_bits).is_none() {
                        dec_ref_bits(_py, parent_path_bits);
                    }
                    let search_paths_bits =
                        match importlib_alloc_string_tuple_bits(_py, &search_paths) {
                            Ok(bits) => bits,
                            Err(err) => {
                                if !obj_from_bits(parent_key_bits).is_none() {
                                    dec_ref_bits(_py, parent_key_bits);
                                }
                                return err;
                            }
                        };
                    (true, false, true, parent_key_bits, search_paths_bits)
                } else {
                    let empty_tuple_ptr = alloc_tuple(_py, &[]);
                    if empty_tuple_ptr.is_null() {
                        if !obj_from_bits(parent_key_bits).is_none() {
                            dec_ref_bits(_py, parent_key_bits);
                        }
                        return raise_exception::<_>(_py, "MemoryError", "out of memory");
                    }
                    (
                        true,
                        true,
                        true,
                        parent_key_bits,
                        MoltObject::from_ptr(empty_tuple_ptr).bits(),
                    )
                }
            } else {
                (
                    false,
                    false,
                    false,
                    MoltObject::none().bits(),
                    MoltObject::none().bits(),
                )
            };

        let has_parent_bits = MoltObject::from_bool(has_parent).bits();
        let needs_parent_spec_bits = MoltObject::from_bool(needs_parent_spec).bits();
        let package_context_bits = MoltObject::from_bool(package_context).bits();

        let keys_and_values: [(&[u8], u64); 5] = [
            (b"has_parent", has_parent_bits),
            (b"parent_name", parent_name_bits),
            (b"search_paths", search_paths_bits),
            (b"needs_parent_spec", needs_parent_spec_bits),
            (b"package_context", package_context_bits),
        ];
        let mut pairs: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        let mut owned: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        for (key, value_bits) in keys_and_values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    if !obj_from_bits(bits).is_none() {
                        dec_ref_bits(_py, bits);
                    }
                }
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            let key_bits = MoltObject::from_ptr(key_ptr).bits();
            pairs.push(key_bits);
            pairs.push(value_bits);
            owned.push(key_bits);
            owned.push(value_bits);
        }
        let payload_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            if !obj_from_bits(bits).is_none() {
                dec_ref_bits(_py, bits);
            }
        }
        if payload_ptr.is_null() {
            raise_exception::<_>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(payload_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_ensure_default_meta_path(machinery_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        static DEFAULT_META_PATH_BOOTSTRAPPED: AtomicU64 = AtomicU64::new(0);
        static META_PATH_NAME: AtomicU64 = AtomicU64::new(0);
        static PATH_FINDER_NAME: AtomicU64 = AtomicU64::new(0);

        let mark_bootstrapped = || {
            DEFAULT_META_PATH_BOOTSTRAPPED.store(1, Ordering::Relaxed);
            MoltObject::none().bits()
        };

        if DEFAULT_META_PATH_BOOTSTRAPPED.load(Ordering::Relaxed) != 0 {
            return MoltObject::none().bits();
        }

        let sys_bits = {
            let module_cache = crate::builtins::exceptions::internals::module_cache(_py);
            let guard = module_cache.lock().unwrap();
            guard.get("sys").copied()
        };
        let Some(sys_bits) = sys_bits else {
            return mark_bootstrapped();
        };
        if obj_from_bits(sys_bits).is_none() {
            return mark_bootstrapped();
        }

        let meta_path_name = intern_static_name(_py, &META_PATH_NAME, b"meta_path");
        let Some(meta_path_bits) = (match getattr_optional_bits(_py, sys_bits, meta_path_name) {
            Ok(value) => value,
            Err(bits) => return bits,
        }) else {
            return mark_bootstrapped();
        };
        let Some(meta_path_ptr) = obj_from_bits(meta_path_bits).as_ptr() else {
            if !obj_from_bits(meta_path_bits).is_none() {
                dec_ref_bits(_py, meta_path_bits);
            }
            return mark_bootstrapped();
        };
        if unsafe { object_type_id(meta_path_ptr) } != TYPE_ID_LIST {
            if !obj_from_bits(meta_path_bits).is_none() {
                dec_ref_bits(_py, meta_path_bits);
            }
            return mark_bootstrapped();
        }
        if unsafe { !seq_vec_ref(meta_path_ptr).is_empty() } {
            if !obj_from_bits(meta_path_bits).is_none() {
                dec_ref_bits(_py, meta_path_bits);
            }
            return mark_bootstrapped();
        }

        let path_finder_name = intern_static_name(_py, &PATH_FINDER_NAME, b"PathFinder");
        let path_finder_bits = match getattr_optional_bits(_py, machinery_bits, path_finder_name) {
            Ok(value) => value,
            Err(bits) => {
                if !obj_from_bits(meta_path_bits).is_none() {
                    dec_ref_bits(_py, meta_path_bits);
                }
                return bits;
            }
        };
        let Some(path_finder_bits) = path_finder_bits else {
            if !obj_from_bits(meta_path_bits).is_none() {
                dec_ref_bits(_py, meta_path_bits);
            }
            return mark_bootstrapped();
        };
        if obj_from_bits(path_finder_bits).is_none() {
            if !obj_from_bits(meta_path_bits).is_none() {
                dec_ref_bits(_py, meta_path_bits);
            }
            return mark_bootstrapped();
        }

        let append_bits = crate::molt_list_append(meta_path_bits, path_finder_bits);
        if !obj_from_bits(append_bits).is_none() {
            dec_ref_bits(_py, append_bits);
        }
        if !obj_from_bits(path_finder_bits).is_none() {
            dec_ref_bits(_py, path_finder_bits);
        }
        if !obj_from_bits(meta_path_bits).is_none() {
            dec_ref_bits(_py, meta_path_bits);
        }
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        mark_bootstrapped()
    })
}

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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
        let is_archive_member_bits = MoltObject::from_bool(payload.is_archive_member).bits();
        let keys_and_values: [(&[u8], u64); 7] = [
            (b"basename", basename_bits),
            (b"exists", exists_bits),
            (b"is_file", is_file_bits),
            (b"is_dir", is_dir_bits),
            (b"entries", entries_bits),
            (b"has_init_py", has_init_py_bits),
            (b"is_archive_member", is_archive_member_bits),
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_package_info(
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
        let tuple_ptr = alloc_tuple(
            _py,
            &[
                roots_bits,
                is_namespace_bits,
                has_regular_package_bits,
                init_file_bits,
            ],
        );
        dec_ref_bits(_py, roots_bits);
        if !obj_from_bits(init_file_bits).is_none() {
            dec_ref_bits(_py, init_file_bits);
        }
        if tuple_ptr.is_null() {
            raise_exception::<_>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(tuple_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_open_resource_bytes_from_package(
    package_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
    resource_bits: u64,
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
        let resource = match string_arg_from_bits(_py, resource_bits, "resource name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let bytes = match importlib_resources_open_resource_bytes_from_package_impl(
            _py,
            &package,
            &search_paths,
            module_file,
            &resource,
        ) {
            Ok(value) => value,
            Err(err) => return err,
        };
        let out_ptr = alloc_bytes(_py, &bytes);
        if out_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_open_resource_bytes_from_package_parts(
    package_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
    path_parts_bits: u64,
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
        let path_parts = match string_sequence_arg_from_bits(_py, path_parts_bits, "path names") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let bytes = match importlib_resources_open_resource_bytes_from_package_parts_impl(
            _py,
            &package,
            &search_paths,
            module_file,
            &path_parts,
        ) {
            Ok(value) => value,
            Err(err) => return err,
        };
        let out_ptr = alloc_bytes(_py, &bytes);
        if out_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        MoltObject::from_ptr(out_ptr).bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_read_text_from_package(
    package_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
    resource_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        static DECODE_NAME: AtomicU64 = AtomicU64::new(0);
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
        let resource = match string_arg_from_bits(_py, resource_bits, "resource name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let encoding = match string_arg_from_bits(_py, encoding_bits, "encoding") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let errors = match string_arg_from_bits(_py, errors_bits, "errors") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let bytes = match importlib_resources_open_resource_bytes_from_package_impl(
            _py,
            &package,
            &search_paths,
            module_file,
            &resource,
        ) {
            Ok(value) => value,
            Err(err) => return err,
        };
        let bytes_ptr = alloc_bytes(_py, &bytes);
        if bytes_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        let bytes_bits = MoltObject::from_ptr(bytes_ptr).bits();
        let decode_name = intern_static_name(_py, &DECODE_NAME, b"decode");
        let decode_bits = match importlib_reader_lookup_callable(_py, bytes_bits, decode_name) {
            Ok(Some(bits)) => bits,
            Ok(None) => {
                dec_ref_bits(_py, bytes_bits);
                return raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "invalid bytes decode callable payload",
                );
            }
            Err(err) => {
                dec_ref_bits(_py, bytes_bits);
                return err;
            }
        };
        let encoding_arg_bits = match alloc_str_bits(_py, &encoding) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, decode_bits);
                dec_ref_bits(_py, bytes_bits);
                return err;
            }
        };
        let errors_arg_bits = match alloc_str_bits(_py, &errors) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, encoding_arg_bits);
                dec_ref_bits(_py, decode_bits);
                dec_ref_bits(_py, bytes_bits);
                return err;
            }
        };
        let out_bits =
            match call_callable_positional(_py, decode_bits, &[encoding_arg_bits, errors_arg_bits])
            {
                Ok(bits) => bits,
                Err(err) => {
                    dec_ref_bits(_py, errors_arg_bits);
                    dec_ref_bits(_py, encoding_arg_bits);
                    dec_ref_bits(_py, decode_bits);
                    dec_ref_bits(_py, bytes_bits);
                    return err;
                }
            };
        dec_ref_bits(_py, errors_arg_bits);
        dec_ref_bits(_py, encoding_arg_bits);
        dec_ref_bits(_py, decode_bits);
        dec_ref_bits(_py, bytes_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        out_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_read_text_from_package_parts(
    package_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
    path_parts_bits: u64,
    encoding_bits: u64,
    errors_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        static DECODE_NAME: AtomicU64 = AtomicU64::new(0);
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
        let path_parts = match string_sequence_arg_from_bits(_py, path_parts_bits, "path names") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let encoding = match string_arg_from_bits(_py, encoding_bits, "encoding") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let errors = match string_arg_from_bits(_py, errors_bits, "errors") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let bytes = match importlib_resources_open_resource_bytes_from_package_parts_impl(
            _py,
            &package,
            &search_paths,
            module_file,
            &path_parts,
        ) {
            Ok(value) => value,
            Err(err) => return err,
        };
        let bytes_ptr = alloc_bytes(_py, &bytes);
        if bytes_ptr.is_null() {
            return raise_exception::<_>(_py, "MemoryError", "out of memory");
        }
        let bytes_bits = MoltObject::from_ptr(bytes_ptr).bits();
        let decode_name = intern_static_name(_py, &DECODE_NAME, b"decode");
        let decode_bits = match importlib_reader_lookup_callable(_py, bytes_bits, decode_name) {
            Ok(Some(bits)) => bits,
            Ok(None) => {
                dec_ref_bits(_py, bytes_bits);
                return raise_exception::<_>(
                    _py,
                    "RuntimeError",
                    "invalid bytes decode callable payload",
                );
            }
            Err(err) => {
                dec_ref_bits(_py, bytes_bits);
                return err;
            }
        };
        let encoding_arg_bits = match alloc_str_bits(_py, &encoding) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, decode_bits);
                dec_ref_bits(_py, bytes_bits);
                return err;
            }
        };
        let errors_arg_bits = match alloc_str_bits(_py, &errors) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, encoding_arg_bits);
                dec_ref_bits(_py, decode_bits);
                dec_ref_bits(_py, bytes_bits);
                return err;
            }
        };
        let out_bits =
            match call_callable_positional(_py, decode_bits, &[encoding_arg_bits, errors_arg_bits])
            {
                Ok(bits) => bits,
                Err(err) => {
                    dec_ref_bits(_py, errors_arg_bits);
                    dec_ref_bits(_py, encoding_arg_bits);
                    dec_ref_bits(_py, decode_bits);
                    dec_ref_bits(_py, bytes_bits);
                    return err;
                }
            };
        dec_ref_bits(_py, errors_arg_bits);
        dec_ref_bits(_py, encoding_arg_bits);
        dec_ref_bits(_py, decode_bits);
        dec_ref_bits(_py, bytes_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        out_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_contents_from_package(
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
        let payload = match importlib_resources_required_package_payload(
            _py,
            &package,
            &search_paths,
            module_file,
        ) {
            Ok(value) => value,
            Err(err) => return err,
        };
        let mut entries: BTreeSet<String> = BTreeSet::new();
        let mut has_init_py = false;
        for root in &payload.roots {
            let root_payload = importlib_resources_path_payload(root);
            has_init_py = has_init_py || root_payload.has_init_py;
            for entry in root_payload.entries {
                entries.insert(entry);
            }
        }
        if has_init_py {
            entries.insert(String::from("__pycache__"));
        }
        let out: Vec<String> = entries.into_iter().collect();
        match alloc_string_list_bits(_py, &out) {
            Some(bits) => bits,
            None => raise_exception::<_>(_py, "MemoryError", "out of memory"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_contents_from_package_parts(
    package_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
    path_parts_bits: u64,
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
        let path_parts = match string_sequence_arg_from_bits(_py, path_parts_bits, "path names") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let out = match importlib_resources_contents_from_package_parts_impl(
            _py,
            &package,
            &search_paths,
            module_file,
            &path_parts,
        ) {
            Ok(value) => value,
            Err(err) => return err,
        };
        match alloc_string_list_bits(_py, &out) {
            Some(bits) => bits,
            None => raise_exception::<_>(_py, "MemoryError", "out of memory"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_is_resource_from_package(
    package_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
    resource_bits: u64,
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
        let resource = match string_arg_from_bits(_py, resource_bits, "resource name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if let Err(err) = importlib_validate_resource_name_text(_py, &resource) {
            return err;
        }
        let payload = match importlib_resources_required_package_payload(
            _py,
            &package,
            &search_paths,
            module_file,
        ) {
            Ok(value) => value,
            Err(err) => return err,
        };
        MoltObject::from_bool(
            importlib_resources_first_file_candidate(&payload.roots, &resource).is_some(),
        )
        .bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_is_resource_from_package_parts(
    package_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
    path_parts_bits: u64,
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
        let path_parts = match string_sequence_arg_from_bits(_py, path_parts_bits, "path names") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match importlib_resources_is_resource_from_package_parts_impl(
            _py,
            &package,
            &search_paths,
            module_file,
            &path_parts,
        ) {
            Ok(value) => MoltObject::from_bool(value).bits(),
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_resource_path_from_package(
    package_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
    resource_bits: u64,
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
        let resource = match string_arg_from_bits(_py, resource_bits, "resource name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if let Err(err) = importlib_validate_resource_name_text(_py, &resource) {
            return err;
        }
        let payload = match importlib_resources_required_package_payload(
            _py,
            &package,
            &search_paths,
            module_file,
        ) {
            Ok(value) => value,
            Err(err) => return err,
        };
        if let Some(candidate) =
            importlib_resources_first_fs_file_candidate(&payload.roots, &resource)
        {
            return match alloc_str_bits(_py, &candidate) {
                Ok(bits) => bits,
                Err(err) => err,
            };
        }
        MoltObject::none().bits()
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_resource_path_from_package_parts(
    package_bits: u64,
    search_paths_bits: u64,
    module_file_bits: u64,
    path_parts_bits: u64,
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
        let path_parts = match string_sequence_arg_from_bits(_py, path_parts_bits, "path names") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match importlib_resources_resource_path_from_package_parts_impl(
            _py,
            &package,
            &search_paths,
            module_file,
            &path_parts,
        ) {
            Ok(Some(path)) => match alloc_str_bits(_py, &path) {
                Ok(bits) => bits,
                Err(err) => err,
            },
            Ok(None) => MoltObject::none().bits(),
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_as_file_enter(
    traversable_bits: u64,
    traversable_type_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        if isinstance_bits(_py, traversable_bits, traversable_type_bits) {
            inc_ref_bits(_py, traversable_bits);
            return traversable_bits;
        }
        let path = match path_from_bits(_py, traversable_bits) {
            Ok(value) => value,
            Err(msg) => return raise_exception::<_>(_py, "TypeError", &msg),
        };
        let path_text = path.to_string_lossy().into_owned();
        let path_bits = match alloc_str_bits(_py, &path_text) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let result_bits = unsafe { call_callable1(_py, traversable_type_bits, path_bits) };
        dec_ref_bits(_py, path_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        result_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_as_file_exit(
    _exc_type_bits: u64,
    _exc_bits: u64,
    _tb_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, { MoltObject::from_bool(false).bits() })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_module_name(
    module_bits: u64,
    fallback_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let name = match importlib_resources_module_name_from_bits(_py, module_bits, fallback_bits)
        {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match alloc_str_bits(_py, &name) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_loader_reader(
    module_bits: u64,
    module_name_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module_name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match importlib_resources_loader_reader_from_bits(_py, module_bits, &module_name) {
            Ok(Some(bits)) => bits,
            Ok(None) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_files_payload(
    module_bits: u64,
    fallback_bits: u64,
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
        let payload = match importlib_resources_files_payload(
            _py,
            module_bits,
            fallback_bits,
            &search_paths,
            module_file,
        ) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let package_name_bits = match alloc_str_bits(_py, &payload.package_name) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let roots_bits = match alloc_string_list_bits(_py, &payload.roots) {
            Some(bits) => bits,
            None => {
                dec_ref_bits(_py, package_name_bits);
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
        };
        let is_namespace_bits = MoltObject::from_bool(payload.is_namespace).bits();
        let reader_bits = payload.reader_bits.unwrap_or(MoltObject::none().bits());
        let files_traversable_bits = payload
            .files_traversable_bits
            .unwrap_or(MoltObject::none().bits());
        let keys_and_values: [(&[u8], u64); 5] = [
            (b"package_name", package_name_bits),
            (b"roots", roots_bits),
            (b"is_namespace", is_namespace_bits),
            (b"reader", reader_bits),
            (b"files_traversable", files_traversable_bits),
        ];
        let mut pairs: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        let mut owned: Vec<u64> = Vec::with_capacity(keys_and_values.len() * 2);
        for (key, value_bits) in keys_and_values {
            let key_ptr = alloc_string(_py, key);
            if key_ptr.is_null() {
                for bits in owned {
                    dec_ref_bits(_py, bits);
                }
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
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
            raise_exception::<_>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(dict_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_reader_files_traversable(reader_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        match importlib_reader_files_traversable_bits(_py, reader_bits) {
            Ok(Some(bits)) => bits,
            Ok(None) => MoltObject::none().bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_reader_roots(reader_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let roots = match importlib_resources_reader_roots_impl(_py, reader_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match alloc_string_list_bits(_py, &roots) {
            Some(bits) => bits,
            None => raise_exception::<_>(_py, "MemoryError", "out of memory"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_reader_contents(reader_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let entries = match importlib_resources_reader_contents_impl(_py, reader_bits) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match alloc_string_list_bits(_py, &entries) {
            Some(bits) => bits,
            None => raise_exception::<_>(_py, "MemoryError", "out of memory"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_reader_resource_path(
    reader_bits: u64,
    name_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let name = match string_arg_from_bits(_py, name_bits, "name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let path = match importlib_resources_reader_resource_path_impl(_py, reader_bits, &name) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match path {
            Some(value) => match alloc_str_bits(_py, &value) {
                Ok(bits) => bits,
                Err(err) => err,
            },
            None => MoltObject::none().bits(),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_reader_is_resource(
    reader_bits: u64,
    name_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let name = match string_arg_from_bits(_py, name_bits, "name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match importlib_resources_reader_is_resource_impl(_py, reader_bits, &name) {
            Ok(value) => MoltObject::from_bool(value).bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_reader_open_resource_bytes(
    reader_bits: u64,
    name_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let name = match string_arg_from_bits(_py, name_bits, "name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let payload =
            match importlib_resources_reader_open_resource_bytes_impl(_py, reader_bits, &name) {
                Ok(value) => value,
                Err(bits) => return bits,
            };
        let out_ptr = alloc_bytes(_py, &payload);
        if out_ptr.is_null() {
            raise_exception::<_>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_reader_child_names(
    reader_bits: u64,
    parts_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        let parts = match string_sequence_arg_from_bits(_py, parts_bits, "parts") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let entries = match importlib_resources_reader_child_names_impl(_py, reader_bits, &parts) {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match alloc_string_list_bits(_py, &entries) {
            Some(bits) => bits,
            None => raise_exception::<_>(_py, "MemoryError", "out of memory"),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_reader_exists(reader_bits: u64, parts_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let parts = match string_sequence_arg_from_bits(_py, parts_bits, "parts") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match importlib_resources_reader_exists_impl(_py, reader_bits, &parts) {
            Ok(value) => MoltObject::from_bool(value).bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_resources_reader_is_dir(reader_bits: u64, parts_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let parts = match string_sequence_arg_from_bits(_py, parts_bits, "parts") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        match importlib_resources_reader_is_dir_impl(_py, reader_bits, &parts) {
            Ok(value) => MoltObject::from_bool(value).bits(),
            Err(bits) => bits,
        }
    })
}

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_metadata_entry_points_filter_payload(
    search_paths_bits: u64,
    module_file_bits: u64,
    group_bits: u64,
    name_bits: u64,
    value_bits: u64,
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
        let value = match optional_string_arg_from_bits(_py, value_bits, "value") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let payload = importlib_metadata_entry_points_filter_payload(
            &search_paths,
            module_file,
            group.as_deref(),
            name.as_deref(),
            value.as_deref(),
        );
        match alloc_string_triplets_list_bits(_py, &payload) {
            Some(bits) => bits,
            None => raise_exception::<_>(_py, "MemoryError", "out of memory"),
        }
    })
}

#[unsafe(no_mangle)]
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

fn alloc_importlib_metadata_payload_dict_bits(
    _py: &PyToken<'_>,
    payload: &ImportlibMetadataPayload,
) -> Result<u64, u64> {
    let path_bits = alloc_str_bits(_py, &payload.path)?;
    let name_bits = match alloc_str_bits(_py, &payload.name) {
        Ok(bits) => bits,
        Err(err) => {
            dec_ref_bits(_py, path_bits);
            return Err(err);
        }
    };
    let version_bits = match alloc_str_bits(_py, &payload.version) {
        Ok(bits) => bits,
        Err(err) => {
            dec_ref_bits(_py, path_bits);
            dec_ref_bits(_py, name_bits);
            return Err(err);
        }
    };
    let metadata_bits = match alloc_string_pairs_dict_bits(_py, &payload.metadata) {
        Some(bits) => bits,
        None => {
            dec_ref_bits(_py, path_bits);
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, version_bits);
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
        }
    };
    let entry_points_bits = match alloc_string_triplets_list_bits(_py, &payload.entry_points) {
        Some(bits) => bits,
        None => {
            dec_ref_bits(_py, path_bits);
            dec_ref_bits(_py, name_bits);
            dec_ref_bits(_py, version_bits);
            dec_ref_bits(_py, metadata_bits);
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
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
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
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
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
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
                return Err(err);
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
            return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
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
        Err(raise_exception::<_>(_py, "MemoryError", "out of memory"))
    } else {
        Ok(MoltObject::from_ptr(dict_ptr).bits())
    }
}

#[unsafe(no_mangle)]
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
        match alloc_importlib_metadata_payload_dict_bits(_py, &payload) {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_metadata_distributions_payload(
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
        let payloads = importlib_metadata_distributions_payload(&search_paths, module_file);
        let mut item_bits: Vec<u64> = Vec::with_capacity(payloads.len());
        for payload in &payloads {
            let bits = match alloc_importlib_metadata_payload_dict_bits(_py, payload) {
                Ok(bits) => bits,
                Err(err) => {
                    for value_bits in item_bits {
                        dec_ref_bits(_py, value_bits);
                    }
                    return err;
                }
            };
            item_bits.push(bits);
        }
        let list_ptr = alloc_list(_py, item_bits.as_slice());
        for bits in item_bits {
            dec_ref_bits(_py, bits);
        }
        if list_ptr.is_null() {
            raise_exception::<_>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_metadata_record_payload(path_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "fs.read") {
            return raise_exception::<_>(_py, "PermissionError", "missing fs.read capability");
        }
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let entries = importlib_metadata_record_payload(&path);
        let mut tuple_bits_vec: Vec<u64> = Vec::with_capacity(entries.len());
        for entry in entries {
            let record_path_bits = match alloc_str_bits(_py, &entry.path) {
                Ok(bits) => bits,
                Err(err) => {
                    for bits in tuple_bits_vec {
                        dec_ref_bits(_py, bits);
                    }
                    return err;
                }
            };
            let hash_bits = match entry.hash.as_deref() {
                Some(value) => match alloc_str_bits(_py, value) {
                    Ok(bits) => bits,
                    Err(err) => {
                        dec_ref_bits(_py, record_path_bits);
                        for bits in tuple_bits_vec {
                            dec_ref_bits(_py, bits);
                        }
                        return err;
                    }
                },
                None => MoltObject::none().bits(),
            };
            let size_bits = match entry.size.as_deref() {
                Some(value) => match alloc_str_bits(_py, value) {
                    Ok(bits) => bits,
                    Err(err) => {
                        if !obj_from_bits(hash_bits).is_none() {
                            dec_ref_bits(_py, hash_bits);
                        }
                        dec_ref_bits(_py, record_path_bits);
                        for bits in tuple_bits_vec {
                            dec_ref_bits(_py, bits);
                        }
                        return err;
                    }
                },
                None => MoltObject::none().bits(),
            };
            let tuple_ptr = alloc_tuple(_py, &[record_path_bits, hash_bits, size_bits]);
            dec_ref_bits(_py, record_path_bits);
            if !obj_from_bits(hash_bits).is_none() {
                dec_ref_bits(_py, hash_bits);
            }
            if !obj_from_bits(size_bits).is_none() {
                dec_ref_bits(_py, size_bits);
            }
            if tuple_ptr.is_null() {
                for bits in tuple_bits_vec {
                    dec_ref_bits(_py, bits);
                }
                return raise_exception::<_>(_py, "MemoryError", "out of memory");
            }
            tuple_bits_vec.push(MoltObject::from_ptr(tuple_ptr).bits());
        }
        let list_ptr = alloc_list(_py, tuple_bits_vec.as_slice());
        for bits in tuple_bits_vec {
            dec_ref_bits(_py, bits);
        }
        if list_ptr.is_null() {
            raise_exception::<_>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(list_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_metadata_packages_distributions_payload(
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
        let payload = importlib_metadata_packages_distributions_payload(&search_paths, module_file);
        let mut pairs: Vec<u64> = Vec::with_capacity(payload.len() * 2);
        let mut owned: Vec<u64> = Vec::with_capacity(payload.len() * 2);
        for (package, providers) in payload {
            let package_bits = match alloc_str_bits(_py, &package) {
                Ok(bits) => bits,
                Err(err) => {
                    for bits in owned {
                        dec_ref_bits(_py, bits);
                    }
                    return err;
                }
            };
            let providers_bits = match alloc_string_list_bits(_py, &providers) {
                Some(bits) => bits,
                None => {
                    dec_ref_bits(_py, package_bits);
                    for bits in owned {
                        dec_ref_bits(_py, bits);
                    }
                    return raise_exception::<_>(_py, "MemoryError", "out of memory");
                }
            };
            pairs.push(package_bits);
            pairs.push(providers_bits);
            owned.push(package_bits);
            owned.push(providers_bits);
        }
        let dict_ptr = alloc_dict_with_pairs(_py, &pairs);
        for bits in owned {
            dec_ref_bits(_py, bits);
        }
        if dict_ptr.is_null() {
            raise_exception::<_>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(dict_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_module_from_spec(spec_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        static LOADER_NAME: AtomicU64 = AtomicU64::new(0);
        static CREATE_MODULE_NAME: AtomicU64 = AtomicU64::new(0);
        static SPEC_ATTR_NAME: AtomicU64 = AtomicU64::new(0);
        static MODULE_NAME_NAME: AtomicU64 = AtomicU64::new(0);
        static PARENT_NAME: AtomicU64 = AtomicU64::new(0);
        static SUBMODULE_SEARCH_LOCATIONS_NAME: AtomicU64 = AtomicU64::new(0);
        static ORIGIN_NAME: AtomicU64 = AtomicU64::new(0);
        static CACHED_NAME: AtomicU64 = AtomicU64::new(0);
        static DUNDER_LOADER_NAME: AtomicU64 = AtomicU64::new(0);
        static DUNDER_PACKAGE_NAME: AtomicU64 = AtomicU64::new(0);
        static DUNDER_PATH_NAME: AtomicU64 = AtomicU64::new(0);
        static DUNDER_FILE_NAME: AtomicU64 = AtomicU64::new(0);
        static DUNDER_CACHED_NAME: AtomicU64 = AtomicU64::new(0);

        let loader_name = intern_static_name(_py, &LOADER_NAME, b"loader");
        let create_module_name = intern_static_name(_py, &CREATE_MODULE_NAME, b"create_module");
        let mut loader_bits = MoltObject::none().bits();
        let mut module_bits = MoltObject::none().bits();
        let out = (|| -> Result<u64, u64> {
            if let Some(bits) = getattr_optional_bits(_py, spec_bits, loader_name)? {
                loader_bits = bits;
                if !obj_from_bits(loader_bits).is_none()
                    && let Some(create_module_bits) =
                        importlib_reader_lookup_callable(_py, loader_bits, create_module_name)?
                {
                    let created_bits =
                        unsafe { call_callable1(_py, create_module_bits, spec_bits) };
                    dec_ref_bits(_py, create_module_bits);
                    if exception_pending(_py) {
                        clear_exception(_py);
                        if !obj_from_bits(created_bits).is_none() {
                            dec_ref_bits(_py, created_bits);
                        }
                    } else if !obj_from_bits(created_bits).is_none() {
                        module_bits = created_bits;
                    }
                }
            }

            if obj_from_bits(module_bits).is_none() {
                let spec_name_bits = importlib_required_attribute(
                    _py,
                    spec_bits,
                    &MODULE_NAME_NAME,
                    b"name",
                    "importlib.machinery.ModuleSpec",
                )?;
                module_bits = crate::molt_module_new(spec_name_bits);
                if !obj_from_bits(spec_name_bits).is_none() {
                    dec_ref_bits(_py, spec_name_bits);
                }
                if exception_pending(_py) {
                    return Err(MoltObject::none().bits());
                }
            }

            importlib_set_attr(_py, module_bits, &SPEC_ATTR_NAME, b"__spec__", spec_bits)?;
            importlib_set_attr(
                _py,
                module_bits,
                &DUNDER_LOADER_NAME,
                b"__loader__",
                loader_bits,
            )?;

            let locations_name = intern_static_name(
                _py,
                &SUBMODULE_SEARCH_LOCATIONS_NAME,
                b"submodule_search_locations",
            );
            let locations_bits = getattr_optional_bits(_py, spec_bits, locations_name)?;
            let has_locations = match locations_bits {
                Some(bits) => {
                    let out = !obj_from_bits(bits).is_none();
                    if !obj_from_bits(bits).is_none() {
                        dec_ref_bits(_py, bits);
                    }
                    out
                }
                None => false,
            };
            if has_locations {
                let spec_name_bits = importlib_required_attribute(
                    _py,
                    spec_bits,
                    &MODULE_NAME_NAME,
                    b"name",
                    "importlib.machinery.ModuleSpec",
                )?;
                importlib_set_attr(
                    _py,
                    module_bits,
                    &DUNDER_PACKAGE_NAME,
                    b"__package__",
                    spec_name_bits,
                )?;
                if !obj_from_bits(spec_name_bits).is_none() {
                    dec_ref_bits(_py, spec_name_bits);
                }

                let locations_bits = importlib_required_attribute(
                    _py,
                    spec_bits,
                    &SUBMODULE_SEARCH_LOCATIONS_NAME,
                    b"submodule_search_locations",
                    "importlib.machinery.ModuleSpec",
                )?;
                let path_bits = importlib_list_from_iterable(
                    _py,
                    locations_bits,
                    "spec.submodule_search_locations",
                )?;
                if !obj_from_bits(locations_bits).is_none() {
                    dec_ref_bits(_py, locations_bits);
                }
                importlib_set_attr(_py, module_bits, &DUNDER_PATH_NAME, b"__path__", path_bits)?;
                if !obj_from_bits(path_bits).is_none() {
                    dec_ref_bits(_py, path_bits);
                }
            } else {
                let parent_bits = importlib_required_attribute(
                    _py,
                    spec_bits,
                    &PARENT_NAME,
                    b"parent",
                    "importlib.machinery.ModuleSpec",
                )?;
                importlib_set_attr(
                    _py,
                    module_bits,
                    &DUNDER_PACKAGE_NAME,
                    b"__package__",
                    parent_bits,
                )?;
                if !obj_from_bits(parent_bits).is_none() {
                    dec_ref_bits(_py, parent_bits);
                }
            }

            let origin_name = intern_static_name(_py, &ORIGIN_NAME, b"origin");
            if let Some(origin_bits) = getattr_optional_bits(_py, spec_bits, origin_name)?
                && !obj_from_bits(origin_bits).is_none()
            {
                importlib_set_attr(
                    _py,
                    module_bits,
                    &DUNDER_FILE_NAME,
                    b"__file__",
                    origin_bits,
                )?;
                dec_ref_bits(_py, origin_bits);
            }

            let cached_bits = importlib_required_attribute(
                _py,
                spec_bits,
                &CACHED_NAME,
                b"cached",
                "importlib.machinery.ModuleSpec",
            )?;
            importlib_set_attr(
                _py,
                module_bits,
                &DUNDER_CACHED_NAME,
                b"__cached__",
                cached_bits,
            )?;
            if !obj_from_bits(cached_bits).is_none() {
                dec_ref_bits(_py, cached_bits);
            }
            Ok(module_bits)
        })();

        if !obj_from_bits(loader_bits).is_none() {
            dec_ref_bits(_py, loader_bits);
        }
        match out {
            Ok(bits) => bits,
            Err(err) => {
                if !obj_from_bits(module_bits).is_none() {
                    dec_ref_bits(_py, module_bits);
                }
                err
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_spec_from_loader(
    name_bits: u64,
    loader_bits: u64,
    origin_bits: u64,
    is_package_bits: u64,
    machinery_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        static MODULE_SPEC_NAME: AtomicU64 = AtomicU64::new(0);
        static IS_PACKAGE_NAME: AtomicU64 = AtomicU64::new(0);
        let _name = match string_arg_from_bits(_py, name_bits, "name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let mut is_package_arg_bits = is_package_bits;
        if obj_from_bits(is_package_bits).is_none() && !obj_from_bits(loader_bits).is_none() {
            let is_package_name = intern_static_name(_py, &IS_PACKAGE_NAME, b"is_package");
            if let Some(call_bits) =
                match importlib_reader_lookup_callable(_py, loader_bits, is_package_name) {
                    Ok(value) => value,
                    Err(bits) => return bits,
                }
            {
                let value_bits = unsafe { call_callable1(_py, call_bits, name_bits) };
                dec_ref_bits(_py, call_bits);
                if exception_pending(_py) {
                    clear_exception(_py);
                } else {
                    let is_package = is_truthy(_py, obj_from_bits(value_bits));
                    if exception_pending(_py) {
                        clear_exception(_py);
                    } else {
                        is_package_arg_bits = MoltObject::from_bool(is_package).bits();
                    }
                }
                if !obj_from_bits(value_bits).is_none() {
                    dec_ref_bits(_py, value_bits);
                }
            }
        }
        let module_spec_cls_bits = match importlib_required_attribute(
            _py,
            machinery_bits,
            &MODULE_SPEC_NAME,
            b"ModuleSpec",
            "importlib.machinery",
        ) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let spec_bits = match call_callable_positional(
            _py,
            module_spec_cls_bits,
            &[name_bits, loader_bits, origin_bits, is_package_arg_bits],
        ) {
            Ok(bits) => bits,
            Err(err) => {
                dec_ref_bits(_py, module_spec_cls_bits);
                return err;
            }
        };
        dec_ref_bits(_py, module_spec_cls_bits);
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        if let Err(err) = importlib_spec_set_cached_from_origin_if_missing(_py, spec_bits) {
            if !obj_from_bits(spec_bits).is_none() {
                dec_ref_bits(_py, spec_bits);
            }
            return err;
        }
        spec_bits
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_spec_from_file_location(
    name_bits: u64,
    location_bits: u64,
    loader_bits: u64,
    submodule_search_locations_bits: u64,
    machinery_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        static MODULE_SPEC_NAME: AtomicU64 = AtomicU64::new(0);
        static SOURCE_FILE_LOADER_NAME: AtomicU64 = AtomicU64::new(0);
        static SUBMODULE_SEARCH_LOCATIONS_NAME: AtomicU64 = AtomicU64::new(0);

        let _name = match string_arg_from_bits(_py, name_bits, "name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let location_text_bits =
            unsafe { call_callable1(_py, builtin_classes(_py).str, location_bits) };
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }
        let path = match string_arg_from_bits(_py, location_text_bits, "location") {
            Ok(value) => value,
            Err(bits) => {
                if !obj_from_bits(location_text_bits).is_none() {
                    dec_ref_bits(_py, location_text_bits);
                }
                return bits;
            }
        };
        if !obj_from_bits(location_text_bits).is_none() {
            dec_ref_bits(_py, location_text_bits);
        }
        let payload = importlib_spec_from_file_location_payload(&path);
        let path_bits = match alloc_str_bits(_py, &payload.path) {
            Ok(bits) => bits,
            Err(err) => return err,
        };

        let mut effective_loader_bits = loader_bits;
        let mut effective_loader_owned = false;
        let mut locations_bits = MoltObject::none().bits();
        let mut locations_owned = false;
        let mut spec_bits = MoltObject::none().bits();
        let out = (|| -> Result<u64, u64> {
            if obj_from_bits(loader_bits).is_none() {
                let loader_cls_bits = importlib_required_attribute(
                    _py,
                    machinery_bits,
                    &SOURCE_FILE_LOADER_NAME,
                    b"SourceFileLoader",
                    "importlib.machinery",
                )?;
                effective_loader_bits =
                    call_callable_positional(_py, loader_cls_bits, &[name_bits, path_bits])?;
                dec_ref_bits(_py, loader_cls_bits);
                if exception_pending(_py) {
                    return Err(MoltObject::none().bits());
                }
                effective_loader_owned = true;
            }

            if !obj_from_bits(submodule_search_locations_bits).is_none() {
                locations_bits = importlib_list_from_iterable(
                    _py,
                    submodule_search_locations_bits,
                    "submodule_search_locations",
                )?;
                locations_owned = true;
            } else if payload.is_package {
                let Some(root) = payload.package_root.as_deref() else {
                    return Err(raise_exception::<_>(
                        _py,
                        "RuntimeError",
                        "invalid importlib spec_from_file_location payload: package_root",
                    ));
                };
                let root_bits = alloc_str_bits(_py, root)?;
                let list_ptr = alloc_list(_py, &[root_bits]);
                if !obj_from_bits(root_bits).is_none() {
                    dec_ref_bits(_py, root_bits);
                }
                if list_ptr.is_null() {
                    return Err(raise_exception::<_>(_py, "MemoryError", "out of memory"));
                }
                locations_bits = MoltObject::from_ptr(list_ptr).bits();
                locations_owned = true;
            }

            let is_package_bits = MoltObject::from_bool(
                payload.is_package || !obj_from_bits(locations_bits).is_none(),
            )
            .bits();
            let module_spec_cls_bits = importlib_required_attribute(
                _py,
                machinery_bits,
                &MODULE_SPEC_NAME,
                b"ModuleSpec",
                "importlib.machinery",
            )?;
            spec_bits = call_callable_positional(
                _py,
                module_spec_cls_bits,
                &[name_bits, effective_loader_bits, path_bits, is_package_bits],
            )?;
            dec_ref_bits(_py, module_spec_cls_bits);
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }

            if !obj_from_bits(locations_bits).is_none() {
                importlib_set_attr(
                    _py,
                    spec_bits,
                    &SUBMODULE_SEARCH_LOCATIONS_NAME,
                    b"submodule_search_locations",
                    locations_bits,
                )?;
            }
            importlib_spec_set_cached_from_origin_if_missing(_py, spec_bits)?;
            Ok(spec_bits)
        })();

        if !obj_from_bits(path_bits).is_none() {
            dec_ref_bits(_py, path_bits);
        }
        if locations_owned && !obj_from_bits(locations_bits).is_none() {
            dec_ref_bits(_py, locations_bits);
        }
        if effective_loader_owned && !obj_from_bits(effective_loader_bits).is_none() {
            dec_ref_bits(_py, effective_loader_bits);
        }
        match out {
            Ok(bits) => bits,
            Err(err) => {
                if !obj_from_bits(spec_bits).is_none() {
                    dec_ref_bits(_py, spec_bits);
                }
                err
            }
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_set_module_state(
    module_bits: u64,
    module_name_bits: u64,
    loader_bits: u64,
    origin_bits: u64,
    is_package_bits: u64,
    module_package_bits: u64,
    package_root_bits: u64,
    module_spec_cls_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        static SPEC_NAME: AtomicU64 = AtomicU64::new(0);
        static MODULE_NAME_NAME: AtomicU64 = AtomicU64::new(0);
        static DUNDER_PATH_NAME: AtomicU64 = AtomicU64::new(0);
        static SUBMODULE_SEARCH_LOCATIONS_NAME: AtomicU64 = AtomicU64::new(0);

        let _module_name = match string_arg_from_bits(_py, module_name_bits, "module_name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let _origin = match string_arg_from_bits(_py, origin_bits, "origin") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let _module_package = match string_arg_from_bits(_py, module_package_bits, "module_package")
        {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let is_package = is_truthy(_py, obj_from_bits(is_package_bits));
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }

        let mut spec_bits = MoltObject::none().bits();
        let mut spec_owned = false;
        let mut module_path_bits = MoltObject::none().bits();
        let mut module_path_owned = false;
        let mut spec_locations_bits = MoltObject::none().bits();
        let mut spec_locations_owned = false;
        let mut modules_bits = MoltObject::none().bits();
        let mut modules_owned = false;

        let out = (|| -> Result<(), u64> {
            let spec_name = intern_static_name(_py, &SPEC_NAME, b"__spec__");
            if let Some(existing_spec_bits) = getattr_optional_bits(_py, module_bits, spec_name)?
                && !obj_from_bits(existing_spec_bits).is_none()
            {
                spec_bits = existing_spec_bits;
                spec_owned = true;
            }

            if obj_from_bits(spec_bits).is_none() {
                spec_bits = call_callable_positional(
                    _py,
                    module_spec_cls_bits,
                    &[
                        module_name_bits,
                        loader_bits,
                        origin_bits,
                        MoltObject::from_bool(is_package).bits(),
                    ],
                )?;
                if exception_pending(_py) {
                    return Err(MoltObject::none().bits());
                }
                importlib_set_attr(_py, module_bits, &SPEC_NAME, b"__spec__", spec_bits)?;
                spec_owned = true;
            } else {
                let module_name_name = intern_static_name(_py, &MODULE_NAME_NAME, b"name");
                let should_fix_name = match getattr_optional_bits(_py, spec_bits, module_name_name)?
                {
                    Some(bits) => {
                        let is_str = string_obj_to_owned(obj_from_bits(bits)).is_some();
                        if !obj_from_bits(bits).is_none() {
                            dec_ref_bits(_py, bits);
                        }
                        !is_str
                    }
                    None => true,
                };
                if should_fix_name
                    && importlib_set_attr(
                        _py,
                        spec_bits,
                        &MODULE_NAME_NAME,
                        b"name",
                        module_name_bits,
                    )
                    .is_err()
                {
                    clear_exception(_py);
                    return Err(raise_exception::<_>(
                        _py,
                        "RuntimeError",
                        "invalid module spec name state",
                    ));
                }
                importlib_spec_set_loader_origin(_py, spec_bits, loader_bits, origin_bits)?;
            }

            importlib_module_set_core_state(
                _py,
                module_bits,
                loader_bits,
                origin_bits,
                module_package_bits,
            )?;

            if is_package {
                importlib_require_package_root_bits(_py, package_root_bits)?;
                module_path_bits = importlib_single_item_list_bits(_py, package_root_bits)?;
                module_path_owned = true;
                importlib_set_attr(
                    _py,
                    module_bits,
                    &DUNDER_PATH_NAME,
                    b"__path__",
                    module_path_bits,
                )?;

                let locations_name = intern_static_name(
                    _py,
                    &SUBMODULE_SEARCH_LOCATIONS_NAME,
                    b"submodule_search_locations",
                );
                let should_set_locations =
                    match getattr_optional_bits(_py, spec_bits, locations_name)? {
                        Some(bits) => {
                            let is_none = obj_from_bits(bits).is_none();
                            if !is_none {
                                dec_ref_bits(_py, bits);
                            }
                            is_none
                        }
                        None => true,
                    };
                if should_set_locations {
                    spec_locations_bits = importlib_single_item_list_bits(_py, package_root_bits)?;
                    spec_locations_owned = true;
                    importlib_set_attr(
                        _py,
                        spec_bits,
                        &SUBMODULE_SEARCH_LOCATIONS_NAME,
                        b"submodule_search_locations",
                        spec_locations_bits,
                    )?;
                }
            }

            modules_bits = importlib_runtime_modules_bits(_py)?;
            modules_owned = true;
            let Some(modules_ptr) = obj_from_bits(modules_bits).as_ptr() else {
                return Err(importlib_modules_runtime_error(_py));
            };
            unsafe {
                dict_set_in_place(_py, modules_ptr, module_name_bits, module_bits);
            }
            if exception_pending(_py) {
                return Err(MoltObject::none().bits());
            }
            Ok(())
        })();

        if modules_owned && !obj_from_bits(modules_bits).is_none() {
            dec_ref_bits(_py, modules_bits);
        }
        if spec_locations_owned && !obj_from_bits(spec_locations_bits).is_none() {
            dec_ref_bits(_py, spec_locations_bits);
        }
        if module_path_owned && !obj_from_bits(module_path_bits).is_none() {
            dec_ref_bits(_py, module_path_bits);
        }
        if spec_owned && !obj_from_bits(spec_bits).is_none() {
            dec_ref_bits(_py, spec_bits);
        }
        match out {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_stabilize_module_state(
    module_bits: u64,
    loader_bits: u64,
    origin_bits: u64,
    is_package_bits: u64,
    module_package_bits: u64,
    package_root_bits: u64,
) -> u64 {
    crate::with_gil_entry!(_py, {
        static SPEC_NAME: AtomicU64 = AtomicU64::new(0);
        static DUNDER_PATH_NAME: AtomicU64 = AtomicU64::new(0);
        static SUBMODULE_SEARCH_LOCATIONS_NAME: AtomicU64 = AtomicU64::new(0);

        let _origin = match string_arg_from_bits(_py, origin_bits, "origin") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let _module_package = match string_arg_from_bits(_py, module_package_bits, "module_package")
        {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let is_package = is_truthy(_py, obj_from_bits(is_package_bits));
        if exception_pending(_py) {
            return MoltObject::none().bits();
        }

        let mut spec_bits = MoltObject::none().bits();
        let mut spec_owned = false;
        let mut module_path_bits = MoltObject::none().bits();
        let mut module_path_owned = false;
        let out = (|| -> Result<(), u64> {
            let spec_name = intern_static_name(_py, &SPEC_NAME, b"__spec__");
            if let Some(existing_spec_bits) = getattr_optional_bits(_py, module_bits, spec_name)?
                && !obj_from_bits(existing_spec_bits).is_none()
            {
                spec_bits = existing_spec_bits;
                spec_owned = true;
                importlib_spec_set_loader_origin(_py, spec_bits, loader_bits, origin_bits)?;
            }

            importlib_module_set_core_state(
                _py,
                module_bits,
                loader_bits,
                origin_bits,
                module_package_bits,
            )?;

            if is_package {
                importlib_require_package_root_bits(_py, package_root_bits)?;
                let dunder_path_name = intern_static_name(_py, &DUNDER_PATH_NAME, b"__path__");
                if let Some(existing_path_bits) =
                    getattr_optional_bits(_py, module_bits, dunder_path_name)?
                {
                    if importlib_is_str_list_bits(existing_path_bits) {
                        module_path_bits = existing_path_bits;
                        module_path_owned = true;
                    } else if !obj_from_bits(existing_path_bits).is_none() {
                        dec_ref_bits(_py, existing_path_bits);
                    }
                }
                if obj_from_bits(module_path_bits).is_none() {
                    module_path_bits = importlib_single_item_list_bits(_py, package_root_bits)?;
                    module_path_owned = true;
                    importlib_set_attr(
                        _py,
                        module_bits,
                        &DUNDER_PATH_NAME,
                        b"__path__",
                        module_path_bits,
                    )?;
                }

                if !obj_from_bits(spec_bits).is_none() {
                    let locations_name = intern_static_name(
                        _py,
                        &SUBMODULE_SEARCH_LOCATIONS_NAME,
                        b"submodule_search_locations",
                    );
                    let should_set_locations =
                        match getattr_optional_bits(_py, spec_bits, locations_name)? {
                            Some(bits) => {
                                let has_valid_locations = importlib_is_str_list_bits(bits);
                                if !obj_from_bits(bits).is_none() {
                                    dec_ref_bits(_py, bits);
                                }
                                !has_valid_locations
                            }
                            None => true,
                        };
                    if should_set_locations {
                        let locations_bits =
                            importlib_list_from_iterable(_py, module_path_bits, "module.__path__")?;
                        let set_out = importlib_set_attr(
                            _py,
                            spec_bits,
                            &SUBMODULE_SEARCH_LOCATIONS_NAME,
                            b"submodule_search_locations",
                            locations_bits,
                        );
                        if !obj_from_bits(locations_bits).is_none() {
                            dec_ref_bits(_py, locations_bits);
                        }
                        set_out?;
                    }
                }
            } else {
                let dunder_path_name = intern_static_name(_py, &DUNDER_PATH_NAME, b"__path__");
                if let Some(module_path_attr_bits) =
                    getattr_optional_bits(_py, module_bits, dunder_path_name)?
                {
                    let should_delete = match obj_from_bits(module_path_attr_bits).as_ptr() {
                        Some(path_ptr) => unsafe { object_type_id(path_ptr) == TYPE_ID_OBJECT },
                        None => false,
                    };
                    if should_delete {
                        let result_bits = crate::molt_object_delattr(module_bits, dunder_path_name);
                        if !obj_from_bits(result_bits).is_none() {
                            dec_ref_bits(_py, result_bits);
                        }
                        if exception_pending(_py) {
                            clear_exception(_py);
                        }
                    }
                    if !obj_from_bits(module_path_attr_bits).is_none() {
                        dec_ref_bits(_py, module_path_attr_bits);
                    }
                }
            }
            Ok(())
        })();

        if module_path_owned && !obj_from_bits(module_path_bits).is_none() {
            dec_ref_bits(_py, module_path_bits);
        }
        if spec_owned && !obj_from_bits(spec_bits).is_none() {
            dec_ref_bits(_py, spec_bits);
        }
        match out {
            Ok(()) => MoltObject::none().bits(),
            Err(err) => err,
        }
    })
}

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_uuid_getnode() -> u64 {
    crate::with_gil_entry!(_py, {
        match uuid_node() {
            Ok(node) => MoltObject::from_int(node as i64).bits(),
            Err(err) => raise_exception::<_>(_py, "RuntimeError", &err),
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_uuid_uuid4_bytes() -> u64 {
    crate::with_gil_entry!(_py, {
        let payload = match uuid_v4_bytes() {
            Ok(bytes) => bytes,
            Err(err) => return raise_exception::<_>(_py, "RuntimeError", &err),
        };
        let out_ptr = alloc_bytes(_py, &payload);
        if out_ptr.is_null() {
            raise_exception::<_>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_uuid_uuid1_bytes(node_bits: u64, clock_seq_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        if !has_capability(_py, "time.wall") && !has_capability(_py, "time") {
            return raise_exception::<_>(_py, "PermissionError", "missing time.wall capability");
        }
        let node_override = if obj_from_bits(node_bits).is_none() {
            None
        } else {
            let value = index_i64_from_obj(_py, node_bits, "node must be an integer");
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if !(0..=0xFFFF_FFFF_FFFF_i64).contains(&value) {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "node is out of range (need a 48-bit value)",
                );
            }
            Some(value as u64)
        };
        let clock_seq_override = if obj_from_bits(clock_seq_bits).is_none() {
            None
        } else {
            let value = index_i64_from_obj(_py, clock_seq_bits, "clock_seq must be an integer");
            if exception_pending(_py) {
                return MoltObject::none().bits();
            }
            if !(0..=0x3FFF_i64).contains(&value) {
                return raise_exception::<_>(
                    _py,
                    "ValueError",
                    "clock_seq is out of range (need a 14-bit value)",
                );
            }
            Some(value as u16)
        };
        let payload = match uuid_v1_bytes(node_override, clock_seq_override) {
            Ok(bytes) => bytes,
            Err(err) => return raise_exception::<_>(_py, "RuntimeError", &err),
        };
        let out_ptr = alloc_bytes(_py, &payload);
        if out_ptr.is_null() {
            raise_exception::<_>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_uuid_uuid3_bytes(namespace_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let namespace = match bytes_arg_from_bits(_py, namespace_bits, "namespace") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if namespace.len() != 16 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "namespace must be a 16-byte UUID payload",
            );
        }
        let name = match bytes_arg_from_bits(_py, name_bits, "name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let payload = uuid_v3_bytes(&namespace, &name);
        let out_ptr = alloc_bytes(_py, &payload);
        if out_ptr.is_null() {
            raise_exception::<_>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_uuid_uuid5_bytes(namespace_bits: u64, name_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        let namespace = match bytes_arg_from_bits(_py, namespace_bits, "namespace") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if namespace.len() != 16 {
            return raise_exception::<_>(
                _py,
                "ValueError",
                "namespace must be a 16-byte UUID payload",
            );
        }
        let name = match bytes_arg_from_bits(_py, name_bits, "name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let payload = uuid_v5_bytes(&namespace, &name);
        let out_ptr = alloc_bytes(_py, &payload);
        if out_ptr.is_null() {
            raise_exception::<_>(_py, "MemoryError", "out of memory")
        } else {
            MoltObject::from_ptr(out_ptr).bits()
        }
    })
}

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
pub extern "C" fn molt_gettext_gettext(message_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        inc_ref_bits(_py, message_bits);
        message_bits
    })
}

#[unsafe(no_mangle)]
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
        // Keep wasm socket constants aligned with run_wasm.js host values so
        // stdlib consumers (e.g. socketserver/smtplib) do not observe missing
        // module attributes.
        vec![
            ("AF_UNIX", libc::AF_UNIX as i64),
            ("AF_INET", libc::AF_INET as i64),
            ("AF_INET6", libc::AF_INET6 as i64),
            ("SOCK_STREAM", libc::SOCK_STREAM as i64),
            ("SOCK_DGRAM", libc::SOCK_DGRAM as i64),
            ("SOCK_RAW", libc::SOCK_RAW as i64),
            ("SOL_SOCKET", libc::SOL_SOCKET as i64),
            ("SO_REUSEADDR", 2),
            ("SO_KEEPALIVE", 9),
            ("SO_SNDBUF", 7),
            ("SO_RCVBUF", 8),
            ("SO_ERROR", 4),
            ("SO_LINGER", 13),
            ("SO_BROADCAST", 6),
            ("SO_REUSEPORT", 15),
            ("IPPROTO_TCP", 6),
            ("IPPROTO_UDP", 17),
            ("IPPROTO_IPV6", 41),
            ("IPV6_V6ONLY", 26),
            ("TCP_NODELAY", 1),
            ("SHUT_RD", 0),
            ("SHUT_WR", 1),
            ("SHUT_RDWR", 2),
            ("AI_PASSIVE", 0x1),
            ("AI_CANONNAME", 0x2),
            ("AI_NUMERICHOST", 0x4),
            ("AI_NUMERICSERV", 0x400),
            ("NI_NUMERICHOST", 0x1),
            ("NI_NUMERICSERV", 0x2),
            ("MSG_PEEK", 2),
            ("MSG_DONTWAIT", libc::MSG_DONTWAIT as i64),
            ("EAI_AGAIN", 2),
            ("EAI_FAIL", 4),
            ("EAI_FAMILY", 5),
            ("EAI_NONAME", libc::EAI_NONAME as i64),
            ("EAI_SERVICE", 9),
            ("EAI_SOCKTYPE", 10),
        ]
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        #[cfg(target_os = "macos")]
        {
            vec![
                ("AF_APPLETALK", 16_i64),
                ("AF_DECnet", 12_i64),
                ("AF_INET", 2_i64),
                ("AF_INET6", 30_i64),
                ("AF_IPX", 23_i64),
                ("AF_LINK", 18_i64),
                ("AF_ROUTE", 17_i64),
                ("AF_SNA", 11_i64),
                ("AF_SYSTEM", 32_i64),
                ("AF_UNIX", 1_i64),
                ("AF_UNSPEC", 0_i64),
                ("AI_ADDRCONFIG", 1024_i64),
                ("AI_ALL", 256_i64),
                ("AI_CANONNAME", 2_i64),
                ("AI_DEFAULT", 1536_i64),
                ("AI_MASK", 5127_i64),
                ("AI_NUMERICHOST", 4_i64),
                ("AI_NUMERICSERV", 4096_i64),
                ("AI_PASSIVE", 1_i64),
                ("AI_V4MAPPED", 2048_i64),
                ("AI_V4MAPPED_CFG", 512_i64),
                ("EAI_ADDRFAMILY", 1_i64),
                ("EAI_AGAIN", 2_i64),
                ("EAI_BADFLAGS", 3_i64),
                ("EAI_BADHINTS", 12_i64),
                ("EAI_FAIL", 4_i64),
                ("EAI_FAMILY", 5_i64),
                ("EAI_MAX", 15_i64),
                ("EAI_MEMORY", 6_i64),
                ("EAI_NODATA", 7_i64),
                ("EAI_NONAME", 8_i64),
                ("EAI_OVERFLOW", 14_i64),
                ("EAI_PROTOCOL", 13_i64),
                ("EAI_SERVICE", 9_i64),
                ("EAI_SOCKTYPE", 10_i64),
                ("EAI_SYSTEM", 11_i64),
                ("ETHERTYPE_ARP", 2054_i64),
                ("ETHERTYPE_IP", 2048_i64),
                ("ETHERTYPE_IPV6", 34525_i64),
                ("ETHERTYPE_VLAN", 33024_i64),
                ("INADDR_ALLHOSTS_GROUP", 3758096385_i64),
                ("INADDR_ANY", 0_i64),
                ("INADDR_BROADCAST", 4294967295_i64),
                ("INADDR_LOOPBACK", 2130706433_i64),
                ("INADDR_MAX_LOCAL_GROUP", 3758096639_i64),
                ("INADDR_NONE", 4294967295_i64),
                ("INADDR_UNSPEC_GROUP", 3758096384_i64),
                ("IPPORT_RESERVED", 1024_i64),
                ("IPPORT_USERRESERVED", 5000_i64),
                ("IPPROTO_AH", 51_i64),
                ("IPPROTO_DSTOPTS", 60_i64),
                ("IPPROTO_EGP", 8_i64),
                ("IPPROTO_EON", 80_i64),
                ("IPPROTO_ESP", 50_i64),
                ("IPPROTO_FRAGMENT", 44_i64),
                ("IPPROTO_GGP", 3_i64),
                ("IPPROTO_GRE", 47_i64),
                ("IPPROTO_HELLO", 63_i64),
                ("IPPROTO_HOPOPTS", 0_i64),
                ("IPPROTO_ICMP", 1_i64),
                ("IPPROTO_ICMPV6", 58_i64),
                ("IPPROTO_IDP", 22_i64),
                ("IPPROTO_IGMP", 2_i64),
                ("IPPROTO_IP", 0_i64),
                ("IPPROTO_IPCOMP", 108_i64),
                ("IPPROTO_IPIP", 4_i64),
                ("IPPROTO_IPV4", 4_i64),
                ("IPPROTO_IPV6", 41_i64),
                ("IPPROTO_MAX", 256_i64),
                ("IPPROTO_ND", 77_i64),
                ("IPPROTO_NONE", 59_i64),
                ("IPPROTO_PIM", 103_i64),
                ("IPPROTO_PUP", 12_i64),
                ("IPPROTO_RAW", 255_i64),
                ("IPPROTO_ROUTING", 43_i64),
                ("IPPROTO_RSVP", 46_i64),
                ("IPPROTO_SCTP", 132_i64),
                ("IPPROTO_TCP", 6_i64),
                ("IPPROTO_TP", 29_i64),
                ("IPPROTO_UDP", 17_i64),
                ("IPPROTO_XTP", 36_i64),
                ("IPV6_CHECKSUM", 26_i64),
                ("IPV6_DONTFRAG", 62_i64),
                ("IPV6_DSTOPTS", 50_i64),
                ("IPV6_HOPLIMIT", 47_i64),
                ("IPV6_HOPOPTS", 49_i64),
                ("IPV6_JOIN_GROUP", 12_i64),
                ("IPV6_LEAVE_GROUP", 13_i64),
                ("IPV6_MULTICAST_HOPS", 10_i64),
                ("IPV6_MULTICAST_IF", 9_i64),
                ("IPV6_MULTICAST_LOOP", 11_i64),
                ("IPV6_NEXTHOP", 48_i64),
                ("IPV6_PATHMTU", 44_i64),
                ("IPV6_PKTINFO", 46_i64),
                ("IPV6_RECVDSTOPTS", 40_i64),
                ("IPV6_RECVHOPLIMIT", 37_i64),
                ("IPV6_RECVHOPOPTS", 39_i64),
                ("IPV6_RECVPATHMTU", 43_i64),
                ("IPV6_RECVPKTINFO", 61_i64),
                ("IPV6_RECVRTHDR", 38_i64),
                ("IPV6_RECVTCLASS", 35_i64),
                ("IPV6_RTHDR", 51_i64),
                ("IPV6_RTHDRDSTOPTS", 57_i64),
                ("IPV6_RTHDR_TYPE_0", 0_i64),
                ("IPV6_TCLASS", 36_i64),
                ("IPV6_UNICAST_HOPS", 4_i64),
                ("IPV6_USE_MIN_MTU", 42_i64),
                ("IPV6_V6ONLY", 27_i64),
                ("IP_ADD_MEMBERSHIP", 12_i64),
                ("IP_ADD_SOURCE_MEMBERSHIP", 70_i64),
                ("IP_BLOCK_SOURCE", 72_i64),
                ("IP_DEFAULT_MULTICAST_LOOP", 1_i64),
                ("IP_DEFAULT_MULTICAST_TTL", 1_i64),
                ("IP_DROP_MEMBERSHIP", 13_i64),
                ("IP_DROP_SOURCE_MEMBERSHIP", 71_i64),
                ("IP_HDRINCL", 2_i64),
                ("IP_MAX_MEMBERSHIPS", 4095_i64),
                ("IP_MULTICAST_IF", 9_i64),
                ("IP_MULTICAST_LOOP", 11_i64),
                ("IP_MULTICAST_TTL", 10_i64),
                ("IP_OPTIONS", 1_i64),
                ("IP_PKTINFO", 26_i64),
                ("IP_RECVDSTADDR", 7_i64),
                ("IP_RECVOPTS", 5_i64),
                ("IP_RECVRETOPTS", 6_i64),
                ("IP_RECVTOS", 27_i64),
                ("IP_RETOPTS", 8_i64),
                ("IP_TOS", 3_i64),
                ("IP_TTL", 4_i64),
                ("IP_UNBLOCK_SOURCE", 73_i64),
                ("LOCAL_PEERCRED", 1_i64),
                ("MSG_CTRUNC", 32_i64),
                ("MSG_DONTROUTE", 4_i64),
                ("MSG_DONTWAIT", 128_i64),
                ("MSG_EOF", 256_i64),
                ("MSG_EOR", 8_i64),
                ("MSG_NOSIGNAL", 524288_i64),
                ("MSG_OOB", 1_i64),
                ("MSG_PEEK", 2_i64),
                ("MSG_TRUNC", 16_i64),
                ("MSG_WAITALL", 64_i64),
                ("NI_DGRAM", 16_i64),
                ("NI_MAXHOST", 1025_i64),
                ("NI_MAXSERV", 32_i64),
                ("NI_NAMEREQD", 4_i64),
                ("NI_NOFQDN", 1_i64),
                ("NI_NUMERICHOST", 2_i64),
                ("NI_NUMERICSERV", 8_i64),
                ("PF_SYSTEM", 32_i64),
                ("SCM_CREDS", 3_i64),
                ("SCM_RIGHTS", 1_i64),
                ("SHUT_RD", 0_i64),
                ("SHUT_RDWR", 2_i64),
                ("SHUT_WR", 1_i64),
                ("SOCK_DGRAM", 2_i64),
                ("SOCK_RAW", 3_i64),
                ("SOCK_RDM", 4_i64),
                ("SOCK_SEQPACKET", 5_i64),
                ("SOCK_STREAM", 1_i64),
                ("SOL_IP", 0_i64),
                ("SOL_SOCKET", 65535_i64),
                ("SOL_TCP", 6_i64),
                ("SOL_UDP", 17_i64),
                ("SOMAXCONN", 128_i64),
                ("SO_ACCEPTCONN", 2_i64),
                ("SO_BINDTODEVICE", 4404_i64),
                ("SO_BROADCAST", 32_i64),
                ("SO_DEBUG", 1_i64),
                ("SO_DONTROUTE", 16_i64),
                ("SO_ERROR", 4103_i64),
                ("SO_KEEPALIVE", 8_i64),
                ("SO_LINGER", 128_i64),
                ("SO_OOBINLINE", 256_i64),
                ("SO_RCVBUF", 4098_i64),
                ("SO_RCVLOWAT", 4100_i64),
                ("SO_RCVTIMEO", 4102_i64),
                ("SO_REUSEADDR", 4_i64),
                ("SO_REUSEPORT", 512_i64),
                ("SO_SNDBUF", 4097_i64),
                ("SO_SNDLOWAT", 4099_i64),
                ("SO_SNDTIMEO", 4101_i64),
                ("SO_TYPE", 4104_i64),
                ("SO_USELOOPBACK", 64_i64),
                ("SYSPROTO_CONTROL", 2_i64),
                ("TCP_CONNECTION_INFO", 262_i64),
                ("TCP_FASTOPEN", 261_i64),
                ("TCP_KEEPALIVE", 16_i64),
                ("TCP_KEEPCNT", 258_i64),
                ("TCP_KEEPINTVL", 257_i64),
                ("TCP_MAXSEG", 2_i64),
                ("TCP_NODELAY", 1_i64),
                ("TCP_NOTSENT_LOWAT", 513_i64),
            ]
        }
        #[cfg(not(target_os = "macos"))]
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
            // AF_ALG constants (kernel crypto API, Linux only)
            #[cfg(target_os = "linux")]
            {
                out.push(("AF_ALG", 38_i64));
                out.push(("SOL_ALG", 279_i64));
                out.push(("ALG_SET_KEY", 1_i64));
                out.push(("ALG_SET_IV", 2_i64));
                out.push(("ALG_SET_OP", 3_i64));
                out.push(("ALG_SET_AEAD_ASSOCLEN", 4_i64));
                out.push(("ALG_SET_AEAD_AUTHSIZE", 5_i64));
                out.push(("ALG_OP_DECRYPT", 0_i64));
                out.push(("ALG_OP_ENCRYPT", 1_i64));
            }
            out
        }
    }
}

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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

#[unsafe(no_mangle)]
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
    use std::collections::BTreeMap;
    use std::io::Write;

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

    fn with_trusted_runtime<R>(f: impl FnOnce() -> R) -> R {
        let _guard = crate::TEST_MUTEX
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let prior = std::env::var("MOLT_TRUSTED").ok();
        unsafe {
            std::env::set_var("MOLT_TRUSTED", "1");
        }
        let _ = crate::state::runtime_state::molt_runtime_shutdown();
        let out = f();
        let _ = crate::state::runtime_state::molt_runtime_shutdown();
        match prior {
            Some(value) => unsafe {
                std::env::set_var("MOLT_TRUSTED", value);
            },
            None => unsafe {
                std::env::remove_var("MOLT_TRUSTED");
            },
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

    fn extension_boundary_temp_dir(prefix: &str) -> std::path::PathBuf {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}_{}_{}", std::process::id(), stamp))
    }

    fn extension_boundary_filename() -> &'static str {
        if sys_platform_str().starts_with("win") {
            "native.pyd"
        } else {
            "native.so"
        }
    }

    fn clear_extension_metadata_validation_cache() {
        let cache = extension_metadata_ok_cache();
        let mut guard = cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.clear();
    }

    fn alloc_test_string_bits(_py: &PyToken<'_>, value: &str) -> u64 {
        let ptr = alloc_string(_py, value.as_bytes());
        assert!(!ptr.is_null(), "alloc string failed for {value:?}");
        MoltObject::from_ptr(ptr).bits()
    }

    fn call_extension_loader_boundary(_py: &PyToken<'_>, module_name: &str, path: &str) -> u64 {
        let module_bits = alloc_test_string_bits(_py, module_name);
        let path_bits = alloc_test_string_bits(_py, path);
        let out = molt_importlib_extension_loader_payload(
            module_bits,
            path_bits,
            MoltObject::from_bool(false).bits(),
        );
        dec_ref_bits(_py, module_bits);
        dec_ref_bits(_py, path_bits);
        out
    }

    fn call_extension_exec_boundary(
        _py: &PyToken<'_>,
        namespace_bits: u64,
        module_name: &str,
        path: &str,
    ) -> u64 {
        let module_bits = alloc_test_string_bits(_py, module_name);
        let path_bits = alloc_test_string_bits(_py, path);
        let out = molt_importlib_exec_extension(namespace_bits, module_bits, path_bits);
        dec_ref_bits(_py, module_bits);
        dec_ref_bits(_py, path_bits);
        out
    }

    fn assert_pending_exception_contains(
        _py: &PyToken<'_>,
        expected_kind: &str,
        fragments: &[&str],
    ) {
        let (kind, message) =
            pending_exception_kind_and_message(_py).expect("expected pending exception");
        assert_eq!(
            kind, expected_kind,
            "unexpected exception kind: {kind} ({message})"
        );
        for fragment in fragments {
            assert!(
                message.contains(fragment),
                "expected fragment {fragment:?} in exception message {message:?}"
            );
        }
        assert!(
            clear_pending_if_kind(_py, &[expected_kind]),
            "failed to clear pending {expected_kind} exception"
        );
        assert!(!exception_pending(_py));
    }

    fn write_valid_extension_manifest(
        manifest_path: &std::path::Path,
        module_name: &str,
        extension_entry: &str,
        extension_sha256: &str,
    ) {
        let abi_major = crate::c_api::MOLT_C_API_VERSION;
        let manifest = serde_json::json!({
            "module": module_name,
            "molt_c_api_version": format!("{abi_major}.0.0"),
            "abi_tag": format!("molt_abi{abi_major}"),
            "target_triple": "test-target",
            "platform_tag": "test-platform",
            "extension": extension_entry,
            "extension_sha256": extension_sha256,
            "capabilities": ["fs.read"],
        });
        let bytes = serde_json::to_vec(&manifest).expect("encode extension manifest");
        std::fs::write(manifest_path, bytes).expect("write extension manifest");
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
                let path_sep = if sys_platform_str().starts_with("win") {
                    '\\'
                } else {
                    '/'
                };
                let expected_alpha =
                    bootstrap_resolve_path_entry("alpha", "/tmp/molt_pwd", path_sep);
                let expected_beta = bootstrap_resolve_path_entry("beta", "/tmp/molt_pwd", path_sep);
                let expected_gamma =
                    bootstrap_resolve_path_entry("gamma", "/tmp/molt_pwd", path_sep);
                let expected_delta =
                    bootstrap_resolve_path_entry("delta", "/tmp/molt_pwd", path_sep);
                assert_eq!(
                    state.pythonpath_entries,
                    vec![expected_alpha.clone(), expected_beta.clone()]
                );
                assert_eq!(
                    state.module_roots_entries,
                    vec![
                        expected_gamma.clone(),
                        expected_beta.clone(),
                        expected_delta.clone()
                    ]
                );
                assert_eq!(state.stdlib_root, Some(expected_stdlib_root()));
                assert_eq!(state.pwd, "/tmp/molt_pwd");
                assert!(state.include_cwd);
                assert_eq!(
                    state.path,
                    vec![
                        "".to_string(),
                        expected_alpha,
                        expected_beta,
                        expected_stdlib_root(),
                        expected_gamma,
                        expected_delta,
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
                assert!(
                    state
                        .venv_site_packages_entries
                        .iter()
                        .any(|entry| entry == &site_packages_text)
                );
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
        let pkg = importlib_find_in_path("pkgdemo", &search_paths, false).expect("package spec");
        assert!(pkg.is_package);
        let pkg_origin = pkg.origin.clone().expect("package origin");
        assert!(pkg_origin.ends_with("__init__.py"));
        assert_eq!(
            pkg.submodule_search_locations,
            Some(vec![pkg_dir.to_string_lossy().into_owned()])
        );
        assert_eq!(pkg.cached, Some(importlib_cache_from_source(&pkg_origin)));
        assert!(pkg.has_location);
        assert_eq!(pkg.loader_kind, "source");

        let module = importlib_find_in_path("moddemo", &search_paths, false).expect("module spec");
        assert!(!module.is_package);
        let module_origin = module.origin.clone().expect("module origin");
        assert!(module_origin.ends_with("moddemo.py"));
        assert_eq!(module.submodule_search_locations, None);
        assert_eq!(
            module.cached,
            Some(importlib_cache_from_source(&module_origin))
        );
        assert!(module.has_location);
        assert_eq!(module.loader_kind, "source");

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    }

    #[test]
    fn importlib_find_in_path_resolves_namespace_package() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_importlib_namespace_spec_{}_{}",
            std::process::id(),
            stamp
        ));
        let left_root = tmp.join("left");
        let right_root = tmp.join("right");
        let left_ns = left_root.join("nspkg");
        let right_ns = right_root.join("nspkg");
        std::fs::create_dir_all(&left_ns).expect("create left namespace path");
        std::fs::create_dir_all(&right_ns).expect("create right namespace path");
        std::fs::write(right_ns.join("mod.py"), "value = 1\n").expect("write module file");

        let search_paths = vec![
            left_root.to_string_lossy().into_owned(),
            right_root.to_string_lossy().into_owned(),
        ];
        let namespace =
            importlib_find_in_path("nspkg", &search_paths, false).expect("namespace spec");
        assert!(namespace.is_package);
        assert_eq!(namespace.origin, None);
        assert_eq!(namespace.cached, None);
        assert!(!namespace.has_location);
        assert_eq!(namespace.loader_kind, "namespace");
        assert_eq!(
            namespace.submodule_search_locations,
            Some(vec![
                left_ns.to_string_lossy().into_owned(),
                right_ns.to_string_lossy().into_owned(),
            ])
        );

        let module =
            importlib_find_in_path("nspkg.mod", &search_paths, false).expect("module spec");
        let module_origin = module.origin.clone().expect("module origin");
        assert!(module_origin.ends_with("mod.py"));
        assert!(!module.is_package);
        assert_eq!(module.loader_kind, "source");

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    }

    #[test]
    fn importlib_find_in_path_resolves_extension_module() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_importlib_extension_spec_{}_{}",
            std::process::id(),
            stamp
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let ext_path = tmp.join("extdemo.so");
        std::fs::write(&ext_path, b"").expect("write extension placeholder");

        let search_paths = vec![tmp.to_string_lossy().into_owned()];
        let module = importlib_find_in_path("extdemo", &search_paths, false).expect("module spec");
        let module_origin = module.origin.clone().expect("module origin");
        assert!(module_origin.ends_with("extdemo.so"));
        assert!(!module.is_package);
        assert_eq!(module.submodule_search_locations, None);
        assert_eq!(module.cached, None);
        assert!(module.has_location);
        assert_eq!(module.loader_kind, "extension");

        std::fs::write(tmp.join("notext.fake.so"), b"").expect("write invalid extension name");
        let missing = importlib_find_in_path("notext", &search_paths, false);
        assert!(missing.is_none());

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    }

    #[test]
    fn extension_path_matches_manifest_entry_variants() {
        assert!(importlib_extension_path_matches_manifest(
            "/tmp/site/demo/native.so",
            "demo/native.so"
        ));
        assert!(importlib_extension_path_matches_manifest(
            "/tmp/site/demo/native.so",
            "native.so"
        ));
        assert!(importlib_extension_path_matches_manifest(
            "C:\\site\\demo\\native.pyd",
            "demo/native.pyd"
        ));
        assert!(!importlib_extension_path_matches_manifest(
            "/tmp/site/demo/other.so",
            "demo/native.so"
        ));
    }

    #[test]
    fn find_extension_manifest_sidecar_walks_parent_dirs() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_extension_manifest_sidecar_{}_{}",
            std::process::id(),
            stamp
        ));
        let pkg_dir = tmp.join("pkg").join("nested");
        std::fs::create_dir_all(&pkg_dir).expect("create extension dir");
        let extension_path = pkg_dir.join("native.so");
        std::fs::write(&extension_path, b"binary").expect("write extension placeholder");
        let manifest_path = tmp.join("extension_manifest.json");
        std::fs::write(&manifest_path, b"{}\n").expect("write manifest");

        let found = importlib_find_extension_manifest_sidecar(&extension_path.to_string_lossy())
            .expect("resolve sidecar");
        assert_eq!(found, Some(manifest_path.to_string_lossy().into_owned()));

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    }

    #[test]
    fn extension_cache_fingerprint_changes_when_binary_changes() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_extension_cache_fingerprint_{}_{}",
            std::process::id(),
            stamp
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let extension_path = tmp.join("native.so");
        std::fs::write(&extension_path, b"abc").expect("write extension");
        let first = importlib_cache_fingerprint_for_path(&extension_path.to_string_lossy())
            .expect("first fingerprint");
        std::fs::write(&extension_path, b"abcdef012345").expect("rewrite extension");
        let second = importlib_cache_fingerprint_for_path(&extension_path.to_string_lossy())
            .expect("second fingerprint");
        assert_ne!(first, second);
        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    }

    #[test]
    fn extension_manifest_cache_fingerprint_changes_when_sidecar_changes() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_manifest_cache_fingerprint_{}_{}",
            std::process::id(),
            stamp
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let manifest_path = tmp.join("extension_manifest.json");
        std::fs::write(&manifest_path, b"{\"module\":\"demo\"}\n").expect("write manifest");
        let loaded = LoadedExtensionManifest {
            source: manifest_path.to_string_lossy().into_owned(),
            manifest: JsonValue::Null,
            wheel_path: None,
        };
        let first = importlib_manifest_cache_fingerprint(&loaded).expect("first fingerprint");
        std::fs::write(
            &manifest_path,
            b"{\"module\":\"demo\",\"capabilities\":[\"fs.read\"]}\n",
        )
        .expect("rewrite manifest");
        let second = importlib_manifest_cache_fingerprint(&loaded).expect("second fingerprint");
        assert_ne!(first, second);
        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    }

    #[test]
    fn extension_loader_boundary_rejects_missing_manifest_sidecar() {
        with_trusted_runtime(|| {
            clear_extension_metadata_validation_cache();
            let tmp = extension_boundary_temp_dir("molt_extension_loader_missing_manifest");
            std::fs::create_dir_all(&tmp).expect("create temp dir");
            let extension_path = tmp.join(extension_boundary_filename());
            std::fs::write(&extension_path, b"loader-boundary-extension")
                .expect("write extension placeholder");
            let module_name = "demo.extension.loader.missing";
            let extension_path_text = extension_path.to_string_lossy().into_owned();

            crate::with_gil_entry!(_py, {
                let _ = call_extension_loader_boundary(_py, module_name, &extension_path_text);
                assert_pending_exception_contains(
                    _py,
                    "ImportError",
                    &[
                        "extension metadata missing",
                        "extension_manifest.json not found near extension path",
                    ],
                );
            });

            std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
        });
    }

    #[test]
    fn extension_loader_boundary_rejects_invalid_manifest_payload() {
        with_trusted_runtime(|| {
            clear_extension_metadata_validation_cache();
            let tmp = extension_boundary_temp_dir("molt_extension_loader_invalid_manifest");
            std::fs::create_dir_all(&tmp).expect("create temp dir");
            let extension_path = tmp.join(extension_boundary_filename());
            std::fs::write(&extension_path, b"loader-boundary-extension")
                .expect("write extension placeholder");
            std::fs::write(tmp.join("extension_manifest.json"), b"{not-json}\n")
                .expect("write invalid manifest");
            let module_name = "demo.extension.loader.invalid";
            let extension_path_text = extension_path.to_string_lossy().into_owned();

            crate::with_gil_entry!(_py, {
                let _ = call_extension_loader_boundary(_py, module_name, &extension_path_text);
                assert_pending_exception_contains(
                    _py,
                    "ImportError",
                    &["invalid extension metadata in", "extension_manifest.json"],
                );
            });

            std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
        });
    }

    #[test]
    fn extension_exec_boundary_rejects_missing_manifest_sidecar() {
        with_trusted_runtime(|| {
            clear_extension_metadata_validation_cache();
            let tmp = extension_boundary_temp_dir("molt_extension_exec_missing_manifest");
            std::fs::create_dir_all(&tmp).expect("create temp dir");
            let extension_path = tmp.join(extension_boundary_filename());
            std::fs::write(&extension_path, b"exec-boundary-extension")
                .expect("write extension placeholder");
            let module_name = "demo.extension.exec.missing";
            let extension_path_text = extension_path.to_string_lossy().into_owned();

            crate::with_gil_entry!(_py, {
                let namespace_ptr = alloc_dict_with_pairs(_py, &[]);
                assert!(!namespace_ptr.is_null(), "alloc namespace dict");
                let namespace_bits = MoltObject::from_ptr(namespace_ptr).bits();
                let _ = call_extension_exec_boundary(
                    _py,
                    namespace_bits,
                    module_name,
                    &extension_path_text,
                );
                dec_ref_bits(_py, namespace_bits);
                assert_pending_exception_contains(
                    _py,
                    "ImportError",
                    &[
                        "extension metadata missing",
                        "extension_manifest.json not found near extension path",
                    ],
                );
            });

            std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
        });
    }

    #[test]
    fn extension_exec_boundary_rejects_invalid_manifest_metadata() {
        with_trusted_runtime(|| {
            clear_extension_metadata_validation_cache();
            let tmp = extension_boundary_temp_dir("molt_extension_exec_invalid_manifest");
            std::fs::create_dir_all(&tmp).expect("create temp dir");
            let extension_path = tmp.join(extension_boundary_filename());
            std::fs::write(&extension_path, b"exec-boundary-extension")
                .expect("write extension placeholder");
            std::fs::write(tmp.join("extension_manifest.json"), b"{}\n")
                .expect("write invalid metadata manifest");
            let module_name = "demo.extension.exec.invalid";
            let extension_path_text = extension_path.to_string_lossy().into_owned();

            crate::with_gil_entry!(_py, {
                let namespace_ptr = alloc_dict_with_pairs(_py, &[]);
                assert!(!namespace_ptr.is_null(), "alloc namespace dict");
                let namespace_bits = MoltObject::from_ptr(namespace_ptr).bits();
                let _ = call_extension_exec_boundary(
                    _py,
                    namespace_bits,
                    module_name,
                    &extension_path_text,
                );
                dec_ref_bits(_py, namespace_bits);
                assert_pending_exception_contains(
                    _py,
                    "ImportError",
                    &["missing or invalid field", "\"module\""],
                );
            });

            std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
        });
    }

    #[test]
    fn extension_loader_boundary_revalidates_cache_after_artifact_mutation() {
        with_trusted_runtime(|| {
            clear_extension_metadata_validation_cache();
            let tmp = extension_boundary_temp_dir("molt_extension_loader_cache_revalidation");
            std::fs::create_dir_all(&tmp).expect("create temp dir");
            let extension_path = tmp.join(extension_boundary_filename());
            let initial_extension = b"extension-v1";
            std::fs::write(&extension_path, initial_extension)
                .expect("write extension placeholder");
            let module_name = "demo.extension.loader.cache";
            let extension_path_text = extension_path.to_string_lossy().into_owned();
            let extension_sha256 =
                importlib_sha256_file(&extension_path_text).expect("hash extension placeholder");
            write_valid_extension_manifest(
                &tmp.join("extension_manifest.json"),
                module_name,
                extension_boundary_filename(),
                &extension_sha256,
            );

            crate::with_gil_entry!(_py, {
                let payload_bits =
                    call_extension_loader_boundary(_py, module_name, &extension_path_text);
                assert!(
                    !exception_pending(_py),
                    "unexpected boundary exception on first pass: {:?}",
                    pending_exception_kind_and_message(_py)
                );
                assert!(
                    !obj_from_bits(payload_bits).is_none(),
                    "expected loader payload on first pass"
                );
                dec_ref_bits(_py, payload_bits);
            });

            std::fs::write(&extension_path, b"extension-v2-with-different-size")
                .expect("mutate extension artifact");

            crate::with_gil_entry!(_py, {
                let _ = call_extension_loader_boundary(_py, module_name, &extension_path_text);
                assert_pending_exception_contains(
                    _py,
                    "ImportError",
                    &["extension checksum mismatch"],
                );
            });

            std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
        });
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn importlib_sha256_path_supports_zip_archive_members() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_importlib_sha_zip_{}_{}",
            std::process::id(),
            stamp
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let archive = tmp.join("mods.whl");
        let file = std::fs::File::create(&archive).expect("create archive");
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::SimpleFileOptions = zip::write::FileOptions::default();
        writer
            .start_file("demo/native.so", options)
            .expect("start archive entry");
        writer
            .write_all(b"zip-extension-bytes")
            .expect("write archive entry");
        writer.finish().expect("finish archive");

        let archive_member_path = format!("{}/demo/native.so", archive.to_string_lossy());
        crate::with_gil_entry!(_py, {
            let digest =
                importlib_sha256_path(_py, &archive_member_path).expect("hash archive member");
            assert_eq!(digest, importlib_sha256_hex(b"zip-extension-bytes"));
        });

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    }

    #[test]
    fn importlib_find_in_path_package_context_resolves_submodule() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_importlib_package_context_{}_{}",
            std::process::id(),
            stamp
        ));
        let pkg_root = tmp.join("pkg");
        std::fs::create_dir_all(&pkg_root).expect("create package root");
        std::fs::write(pkg_root.join("mod.py"), "value = 3\n").expect("write module file");

        let search_paths = vec![pkg_root.to_string_lossy().into_owned()];
        let module = importlib_find_in_path("pkg.mod", &search_paths, true).expect("module spec");
        let module_origin = module.origin.clone().expect("module origin");
        assert!(module_origin.ends_with("mod.py"));
        assert!(!module.is_package);
        assert_eq!(module.loader_kind, "source");

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    }

    #[test]
    fn importlib_find_in_path_resolves_sourceless_bytecode() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_importlib_bytecode_spec_{}_{}",
            std::process::id(),
            stamp
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        std::fs::write(tmp.join("bcmod.pyc"), b"bytecode").expect("write module bytecode");

        let pkg_dir = tmp.join("bcpkg");
        std::fs::create_dir_all(&pkg_dir).expect("create package dir");
        std::fs::write(pkg_dir.join("__init__.pyc"), b"bytecode").expect("write package bytecode");

        let search_paths = vec![tmp.to_string_lossy().into_owned()];
        let module = importlib_find_in_path("bcmod", &search_paths, false).expect("module spec");
        assert_eq!(module.loader_kind, "bytecode");
        assert_eq!(module.cached, None);
        assert!(
            module
                .origin
                .as_deref()
                .unwrap_or("")
                .ends_with("bcmod.pyc")
        );

        let package = importlib_find_in_path("bcpkg", &search_paths, false).expect("package spec");
        assert_eq!(package.loader_kind, "bytecode");
        assert!(package.is_package);
        assert_eq!(
            package.submodule_search_locations,
            Some(vec![pkg_dir.to_string_lossy().into_owned()])
        );

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn importlib_find_in_path_resolves_zip_source_module_and_package() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_importlib_zip_source_spec_{}_{}",
            std::process::id(),
            stamp
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let archive = tmp.join("mods.zip");
        let file = std::fs::File::create(&archive).expect("create zip file");
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::SimpleFileOptions =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        writer
            .start_file("zipmod.py", options)
            .expect("start module zip entry");
        writer
            .write_all(b"value = 11\n")
            .expect("write module source");
        writer
            .start_file("zpkg/__init__.py", options)
            .expect("start package zip entry");
        writer
            .write_all(b"flag = 7\n")
            .expect("write package source");
        writer.finish().expect("finish zip file");

        let archive_text = archive.to_string_lossy().into_owned();
        let search_paths = vec![archive_text.clone()];
        let module = importlib_find_in_path("zipmod", &search_paths, false).expect("module spec");
        assert_eq!(module.loader_kind, "zip_source");
        assert_eq!(module.zip_archive, Some(archive_text.clone()));
        assert_eq!(module.zip_inner_path, Some("zipmod.py".to_string()));
        assert!(
            module
                .origin
                .as_deref()
                .unwrap_or("")
                .ends_with("mods.zip/zipmod.py")
        );

        let package = importlib_find_in_path("zpkg", &search_paths, false).expect("package spec");
        assert_eq!(package.loader_kind, "zip_source");
        assert!(package.is_package);
        assert_eq!(package.zip_archive, Some(archive_text.clone()));
        assert_eq!(package.zip_inner_path, Some("zpkg/__init__.py".to_string()));
        assert_eq!(
            package.submodule_search_locations,
            Some(vec![format!("{archive_text}/zpkg")])
        );

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn importlib_zip_source_exec_payload_reads_source_and_resolution() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_importlib_zip_exec_payload_{}_{}",
            std::process::id(),
            stamp
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let archive = tmp.join("mods.zip");
        let file = std::fs::File::create(&archive).expect("create zip file");
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::SimpleFileOptions =
            zip::write::FileOptions::default().compression_method(zip::CompressionMethod::Stored);
        writer
            .start_file("zipmod.py", options)
            .expect("start module zip entry");
        writer
            .write_all(b"value = 41\n")
            .expect("write module source");
        writer.finish().expect("finish zip file");

        let archive_text = archive.to_string_lossy().into_owned();
        let payload =
            importlib_zip_source_exec_payload("zipmod", &archive_text, "zipmod.py", false)
                .expect("build zip source exec payload");
        assert!(!payload.is_package);
        assert_eq!(payload.module_package, "");
        assert_eq!(payload.package_root, None);
        assert!(payload.origin.ends_with("mods.zip/zipmod.py"));
        let text = String::from_utf8(payload.source.clone()).expect("decode source text");
        assert!(text.contains("value = 41"));

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
                    importlib_search_paths(&["src".to_string()], Some(bootstrap_module_file()));
                assert!(resolved.iter().any(|entry| entry == "src"));
                let path_sep = if sys_platform_str().starts_with("win") {
                    '\\'
                } else {
                    '/'
                };
                let expected_vendor =
                    bootstrap_resolve_path_entry("vendor", "/tmp/bootstrap_pwd", path_sep);
                let expected_extra =
                    bootstrap_resolve_path_entry("extra", "/tmp/bootstrap_pwd", path_sep);
                assert!(resolved.iter().any(|entry| entry == &expected_vendor));
                assert!(resolved.iter().any(|entry| entry == &expected_extra));
                assert!(resolved.iter().any(|entry| {
                    entry.ends_with("/molt/stdlib") || entry.ends_with("\\molt\\stdlib")
                }));
                assert!(
                    resolved
                        .iter()
                        .any(|entry| entry == &expected_stdlib_root())
                );
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
    #[cfg_attr(miri, ignore)]
    fn importlib_namespace_paths_finds_zip_namespace_dirs() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_importlib_namespace_zip_paths_{}_{}",
            std::process::id(),
            stamp
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let archive = tmp.join("mods.zip");
        let file = std::fs::File::create(&archive).expect("create zip file");
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::SimpleFileOptions = zip::write::FileOptions::default();
        writer
            .start_file("nszip/pkg/mod.py", options)
            .expect("start namespace file");
        writer
            .write_all(b"value = 1\n")
            .expect("write namespace file");
        writer.finish().expect("finish zip archive");

        let archive_text = archive.to_string_lossy().into_owned();
        let expected = format!("{archive_text}/nszip/pkg");
        let resolved =
            importlib_namespace_paths("nszip.pkg", &[archive_text], Some(bootstrap_module_file()));
        assert!(resolved.iter().any(|entry| entry == &expected));

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
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
            &[tmp.to_string_lossy().into_owned()],
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
        assert!(!payload.is_archive_member);
        assert!(payload.entries.iter().any(|entry| entry == "__init__.py"));
        assert!(payload.entries.iter().any(|entry| entry == "data.txt"));

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn importlib_resources_zip_payload_reports_entries_and_init_marker() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_importlib_resources_zip_payload_{}_{}",
            std::process::id(),
            stamp
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let archive = tmp.join("resources.zip");
        let file = std::fs::File::create(&archive).expect("create zip file");
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::SimpleFileOptions = zip::write::FileOptions::default();
        writer
            .start_file("pkg/__init__.py", options)
            .expect("start __init__.py");
        writer
            .write_all(b"x = 1\n")
            .expect("write __init__.py in zip");
        writer
            .start_file("pkg/data.txt", options)
            .expect("start data.txt");
        writer
            .write_all(b"payload\n")
            .expect("write data.txt in zip");
        writer.finish().expect("finish zip archive");

        let archive_text = archive.to_string_lossy().into_owned();
        let package_root = format!("{archive_text}/pkg");
        let package_payload = importlib_resources_path_payload(&package_root);
        assert!(package_payload.exists);
        assert!(package_payload.is_dir);
        assert!(!package_payload.is_file);
        assert!(package_payload.has_init_py);
        assert!(package_payload.is_archive_member);
        assert!(
            package_payload
                .entries
                .iter()
                .any(|entry| entry == "__init__.py")
        );
        assert!(
            package_payload
                .entries
                .iter()
                .any(|entry| entry == "data.txt")
        );

        let file_payload = importlib_resources_path_payload(&format!("{package_root}/data.txt"));
        assert!(file_payload.exists);
        assert!(file_payload.is_file);
        assert!(!file_payload.is_dir);
        assert!(!file_payload.has_init_py);
        assert!(file_payload.is_archive_member);

        let package_meta = importlib_resources_package_payload(
            "pkg",
            std::slice::from_ref(&archive_text),
            Some(bootstrap_module_file()),
        );
        assert!(package_meta.has_regular_package);
        assert!(
            package_meta
                .roots
                .iter()
                .any(|entry| entry == &package_root)
        );
        assert!(
            package_meta
                .init_file
                .as_deref()
                .is_some_and(|entry| entry.ends_with("resources.zip/pkg/__init__.py"))
        );

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    }

    #[test]
    #[cfg_attr(miri, ignore)]
    fn importlib_resources_whl_payload_reports_archive_member_flag() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_importlib_resources_whl_payload_{}_{}",
            std::process::id(),
            stamp
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let archive = tmp.join("resources.whl");
        let file = std::fs::File::create(&archive).expect("create whl file");
        let mut writer = zip::ZipWriter::new(file);
        let options: zip::write::SimpleFileOptions = zip::write::FileOptions::default();
        writer
            .start_file("pkg/data.txt", options)
            .expect("start data.txt");
        writer
            .write_all(b"payload\n")
            .expect("write data.txt in whl");
        writer.finish().expect("finish whl archive");

        let archive_text = archive.to_string_lossy().into_owned();
        let file_payload =
            importlib_resources_path_payload(&format!("{archive_text}/pkg/data.txt"));
        assert!(file_payload.exists);
        assert!(file_payload.is_file);
        assert!(file_payload.is_archive_member);

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
        assert!(
            payload
                .metadata
                .iter()
                .any(|(key, value)| key == "Name" && value == "demo-pkg")
        );
        assert!(payload.entry_points.iter().any(|(name, value, group)| {
            name == "demo" && value == "demo_pkg:main" && group == "console_scripts"
        }));
        assert_eq!(payload.requires_python.as_deref(), Some(">=3.12"));
        assert_eq!(payload.requires_dist.len(), 2);
        assert!(
            payload
                .requires_dist
                .iter()
                .any(|value| value == "dep-one>=1")
        );
        assert!(
            payload
                .requires_dist
                .iter()
                .any(|value| value == "dep-two; extra == \"dev\"")
        );
        assert!(payload.provides_extra.iter().any(|value| value == "dev"));

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    }

    #[test]
    fn importlib_metadata_record_payload_parses_rows() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_importlib_metadata_record_payload_{}_{}",
            std::process::id(),
            stamp
        ));
        let dist = tmp.join("demo_record-1.0.dist-info");
        std::fs::create_dir_all(&dist).expect("create dist-info dir");
        std::fs::write(
            dist.join("RECORD"),
            "demo_record/__init__.py,sha256=abc123,17\n\"demo_record/data,file.txt\",,\n",
        )
        .expect("write RECORD");

        let payload = importlib_metadata_record_payload(&dist.to_string_lossy());
        assert_eq!(payload.len(), 2);
        assert_eq!(payload[0].path, "demo_record/__init__.py");
        assert_eq!(payload[0].hash.as_deref(), Some("sha256=abc123"));
        assert_eq!(payload[0].size.as_deref(), Some("17"));
        assert_eq!(payload[1].path, "demo_record/data,file.txt");
        assert!(payload[1].hash.is_none());
        assert!(payload[1].size.is_none());

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    }

    #[test]
    fn importlib_metadata_packages_distributions_payload_aggregates_top_level() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_importlib_metadata_packages_payload_{}_{}",
            std::process::id(),
            stamp
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let dist_one = tmp.join("demo_one-1.0.dist-info");
        let dist_two = tmp.join("demo_two-2.0.dist-info");
        std::fs::create_dir_all(&dist_one).expect("create dist one");
        std::fs::create_dir_all(&dist_two).expect("create dist two");
        std::fs::write(dist_one.join("METADATA"), "Name: demo-one\nVersion: 1.0\n")
            .expect("write metadata one");
        std::fs::write(dist_two.join("METADATA"), "Name: demo-two\nVersion: 2.0\n")
            .expect("write metadata two");
        std::fs::write(
            dist_one.join("top_level.txt"),
            "pkg_one\npkg_shared\npkg_shared\n",
        )
        .expect("write top_level one");
        std::fs::write(dist_two.join("top_level.txt"), "pkg_two\npkg_shared\n")
            .expect("write top_level two");

        let payload = importlib_metadata_packages_distributions_payload(
            &[tmp.to_string_lossy().into_owned()],
            Some(bootstrap_module_file()),
        );
        let mapping: BTreeMap<String, Vec<String>> = payload.into_iter().collect();
        assert_eq!(mapping.get("pkg_one"), Some(&vec!["demo-one".to_string()]));
        assert_eq!(mapping.get("pkg_two"), Some(&vec!["demo-two".to_string()]));
        assert_eq!(
            mapping.get("pkg_shared"),
            Some(&vec!["demo-one".to_string(), "demo-two".to_string()])
        );

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
                    &["src".to_string()],
                    Some(bootstrap_module_file()),
                );
                let path_sep = if sys_platform_str().starts_with("win") {
                    '\\'
                } else {
                    '/'
                };
                let expected_alpha =
                    bootstrap_resolve_path_entry("alpha", "/tmp/bootstrap_pwd", path_sep);
                let expected_vendor =
                    bootstrap_resolve_path_entry("vendor", "/tmp/bootstrap_pwd", path_sep);
                assert!(
                    payload
                        .resolved_search_paths
                        .iter()
                        .any(|entry| entry == "src")
                );
                assert!(
                    payload
                        .resolved_search_paths
                        .iter()
                        .any(|entry| entry == &expected_stdlib_root())
                );
                assert_eq!(payload.pythonpath_entries, vec![expected_alpha]);
                assert!(
                    payload
                        .module_roots_entries
                        .iter()
                        .any(|entry| entry == &expected_vendor)
                );
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
            &[tmp.to_string_lossy().into_owned()],
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
        assert!(
            group_filtered
                .iter()
                .all(|(_, _, group)| group == "console_scripts")
        );
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
    fn importlib_metadata_entry_points_filter_payload_filters_by_value() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_importlib_metadata_entry_points_filter_payload_{}_{}",
            std::process::id(),
            stamp
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let dist = tmp.join("demo_filter-1.0.dist-info");
        std::fs::create_dir_all(&dist).expect("create dist");
        std::fs::write(
            dist.join("entry_points.txt"),
            "[demo.group]\nalpha = demo:alpha\nbeta = demo:beta\n",
        )
        .expect("write entry points");
        let search_paths = vec![tmp.to_string_lossy().into_owned()];
        let filtered = importlib_metadata_entry_points_filter_payload(
            &search_paths,
            Some(bootstrap_module_file()),
            Some("demo.group"),
            None,
            Some("demo:beta"),
        );
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].0, "beta");
        assert_eq!(filtered[0].1, "demo:beta");
        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    }

    #[test]
    fn importlib_metadata_distributions_payload_aggregates_dist_payloads() {
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let tmp = std::env::temp_dir().join(format!(
            "molt_importlib_metadata_distributions_payload_{}_{}",
            std::process::id(),
            stamp
        ));
        std::fs::create_dir_all(&tmp).expect("create temp dir");
        let dist_one = tmp.join("demo_bulk_one-1.0.dist-info");
        let dist_two = tmp.join("demo_bulk_two-2.0.dist-info");
        std::fs::create_dir_all(&dist_one).expect("create dist one");
        std::fs::create_dir_all(&dist_two).expect("create dist two");
        std::fs::write(
            dist_one.join("METADATA"),
            "Name: demo-bulk-one\nVersion: 1.0\n",
        )
        .expect("write metadata one");
        std::fs::write(
            dist_two.join("METADATA"),
            "Name: demo-bulk-two\nVersion: 2.0\n",
        )
        .expect("write metadata two");

        let payloads = importlib_metadata_distributions_payload(
            &[tmp.to_string_lossy().into_owned()],
            Some(bootstrap_module_file()),
        );
        assert_eq!(payloads.len(), 2);
        assert!(
            payloads
                .iter()
                .any(|payload| payload.name == "demo-bulk-one" && payload.version == "1.0")
        );
        assert!(
            payloads
                .iter()
                .any(|payload| payload.name == "demo-bulk-two" && payload.version == "2.0")
        );

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
unsafe extern "C" {
    fn environ_sizes_get(environ_count: *mut u32, environ_buf_size: *mut u32) -> u16;
    fn environ_get(environ: *mut *mut u8, environ_buf: *mut u8) -> u16;
}
