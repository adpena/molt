use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::io::Read;
use std::path::Path;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use digest::Digest;
use md5::Md5;
use serde_json::Value as JsonValue;
use sha1::Sha1;
use sha2::Sha256;

use crate::audit::{AuditArgs, AuditDecision, AuditEvent, audit_capability_decision, audit_emit};
use crate::builtins::io::{
    path_basename_text, path_dirname_text, path_join_text, path_normpath_text,
};
use crate::builtins::modules::runpy_exec_restricted_source;
#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
use crate::randomness::fill_os_random;
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
static EXTENSION_METADATA_CACHE_HITS: AtomicU64 = AtomicU64::new(0);
static EXTENSION_METADATA_CACHE_MISSES: AtomicU64 = AtomicU64::new(0);
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

#[cfg(test)]
fn extension_metadata_cache_stats() -> (u64, u64) {
    (
        EXTENSION_METADATA_CACHE_HITS.load(Ordering::Relaxed),
        EXTENSION_METADATA_CACHE_MISSES.load(Ordering::Relaxed),
    )
}

fn uuid_random_bytes<const N: usize>() -> Result<[u8; N], String> {
    let mut out = [0u8; N];
    fill_os_random(&mut out).map_err(|err| format!("os randomness unavailable: {err}"))?;
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

#[cfg(all(target_arch = "wasm32", not(feature = "wasm_freestanding")))]
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

#[cfg(all(target_arch = "wasm32", feature = "wasm_freestanding"))]
fn collect_wasm_env_state() -> BTreeMap<String, String> {
    BTreeMap::new()
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

fn append_unique_path_hashed(paths: &mut Vec<String>, seen: &mut HashSet<String>, entry: &str) {
    if entry.is_empty() {
        return;
    }
    if seen.insert(entry.to_string()) {
        paths.push(entry.to_string());
    }
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
    let mut seen: HashSet<String> = HashSet::new();
    if virtual_env.trim().is_empty() {
        return out;
    }
    let sep = if windows_paths { '\\' } else { '/' };
    let virtual_env = virtual_env.trim();
    if windows_paths {
        let lib = path_join_text(virtual_env.to_string(), "Lib", sep);
        let site_packages = path_join_text(lib, "site-packages", sep);
        if path_is_dir(&site_packages) {
            append_unique_path_hashed(&mut out, &mut seen, &site_packages);
        }
        let lib_lower = path_join_text(virtual_env.to_string(), "lib", sep);
        let site_packages_lower = path_join_text(lib_lower, "site-packages", sep);
        if path_is_dir(&site_packages_lower) {
            append_unique_path_hashed(&mut out, &mut seen, &site_packages_lower);
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
        append_unique_path_hashed(&mut out, &mut seen, &candidate);
    }
    let fallback = path_join_text(lib, "site-packages", sep);
    if path_is_dir(&fallback) {
        append_unique_path_hashed(&mut out, &mut seen, &fallback);
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
    let mut pythonpath_seen: HashSet<String> = HashSet::new();
    for entry in split_nonempty_paths(&py_path_raw, sep) {
        let resolved = bootstrap_resolve_path_entry(&entry, &pwd, path_sep);
        append_unique_path_hashed(&mut pythonpath_entries, &mut pythonpath_seen, &resolved);
    }
    let mut paths: Vec<String> = pythonpath_entries.clone();
    let mut paths_seen: HashSet<String> = pythonpath_entries.iter().cloned().collect();

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
        append_unique_path_hashed(&mut paths, &mut paths_seen, root);
    }

    let mut module_roots_entries: Vec<String> = Vec::new();
    let mut module_roots_seen: HashSet<String> = HashSet::new();
    for entry in split_nonempty_paths(&module_roots_raw, sep) {
        let resolved = bootstrap_resolve_path_entry(&entry, &pwd, path_sep);
        append_unique_path_hashed(&mut module_roots_entries, &mut module_roots_seen, &resolved);
        append_unique_path_hashed(&mut paths, &mut paths_seen, &resolved);
    }

    let venv_site_packages_entries =
        collect_virtual_env_site_packages(&virtual_env_raw, windows_paths);
    for entry in &venv_site_packages_entries {
        append_unique_path_hashed(&mut paths, &mut paths_seen, entry);
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

#[cfg(feature = "stdlib_archive")]
fn zip_archive_open(path: &str) -> Result<zip::ZipArchive<std::fs::File>, std::io::Error> {
    let file = std::fs::File::open(path)?;
    zip::ZipArchive::new(file)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::InvalidData, err.to_string()))
}

#[cfg(feature = "stdlib_archive")]
fn zip_archive_entry_exists(path: &str, entry: &str) -> bool {
    let Ok(mut archive) = zip_archive_open(path) else {
        return false;
    };

    archive.by_name(entry).is_ok()
}

#[cfg(feature = "stdlib_archive")]
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

#[derive(Default)]
#[cfg(feature = "stdlib_archive")]
struct ZipArchiveIndex {
    entries: HashSet<String>,
    prefixes: HashSet<String>,
}

#[cfg(feature = "stdlib_archive")]
fn zip_archive_build_index(path: &str) -> Option<ZipArchiveIndex> {
    let mut archive = zip_archive_open(path).ok()?;
    let mut entries: HashSet<String> = HashSet::new();
    let mut prefixes: HashSet<String> = HashSet::new();
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
        entries.insert(name.clone());
        if is_dir_entry {
            prefixes.insert(name.clone());
        }
        let mut cursor = name.as_str();
        while let Some((parent, _)) = cursor.rsplit_once('/') {
            if parent.is_empty() {
                break;
            }
            prefixes.insert(parent.to_string());
            cursor = parent;
        }
    }
    Some(ZipArchiveIndex { entries, prefixes })
}

#[cfg(feature = "stdlib_archive")]
fn zip_archive_index_cached<'a>(
    cache: &'a mut HashMap<String, Option<ZipArchiveIndex>>,
    path: &str,
) -> Option<&'a ZipArchiveIndex> {
    cache
        .entry(path.to_string())
        .or_insert_with(|| zip_archive_build_index(path))
        .as_ref()
}

#[cfg(feature = "stdlib_archive")]
fn zip_archive_entry_exists_cached(
    cache: &mut HashMap<String, Option<ZipArchiveIndex>>,
    path: &str,
    entry: &str,
) -> bool {
    let normalized = entry.replace('\\', "/").trim_matches('/').to_string();
    if normalized.is_empty() {
        return false;
    }
    zip_archive_index_cached(cache, path)
        .map(|index| index.entries.contains(&normalized))
        .unwrap_or(false)
}

#[cfg(feature = "stdlib_archive")]
fn zip_archive_has_prefix_cached(
    cache: &mut HashMap<String, Option<ZipArchiveIndex>>,
    path: &str,
    prefix: &str,
) -> bool {
    let normalized = prefix.replace('\\', "/").trim_matches('/').to_string();
    if normalized.is_empty() {
        return false;
    }
    zip_archive_index_cached(cache, path)
        .map(|index| index.prefixes.contains(&normalized))
        .unwrap_or(false)
}

#[cfg(feature = "stdlib_archive")]
fn zip_archive_read_entry(path: &str, entry: &str) -> Result<Vec<u8>, std::io::Error> {
    let mut archive = zip_archive_open(path)?;
    let mut file = archive
        .by_name(entry)
        .map_err(|err| std::io::Error::new(std::io::ErrorKind::NotFound, err.to_string()))?;
    let mut bytes: Vec<u8> = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

#[cfg(feature = "stdlib_archive")]
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

#[cfg(feature = "stdlib_archive")]
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

// --- Stubs when stdlib_archive is disabled ---

#[cfg(not(feature = "stdlib_archive"))]
fn zip_archive_entry_exists(_path: &str, _entry: &str) -> bool {
    false
}

#[cfg(not(feature = "stdlib_archive"))]
fn zip_archive_has_prefix(_path: &str, _prefix: &str) -> bool {
    false
}

#[cfg(not(feature = "stdlib_archive"))]
#[derive(Default)]
struct ZipArchiveIndex {
    entries: HashSet<String>,
    prefixes: HashSet<String>,
}

#[cfg(not(feature = "stdlib_archive"))]
fn zip_archive_build_index(_path: &str) -> Option<ZipArchiveIndex> {
    None
}

#[cfg(not(feature = "stdlib_archive"))]
fn zip_archive_index_cached<'a>(
    cache: &'a mut HashMap<String, Option<ZipArchiveIndex>>,
    path: &str,
) -> Option<&'a ZipArchiveIndex> {
    cache.entry(path.to_string()).or_insert(None).as_ref()
}

#[cfg(not(feature = "stdlib_archive"))]
fn zip_archive_entry_exists_cached(
    _cache: &mut HashMap<String, Option<ZipArchiveIndex>>,
    _path: &str,
    _entry: &str,
) -> bool {
    false
}

#[cfg(not(feature = "stdlib_archive"))]
fn zip_archive_has_prefix_cached(
    _cache: &mut HashMap<String, Option<ZipArchiveIndex>>,
    _path: &str,
    _prefix: &str,
) -> bool {
    false
}

#[cfg(not(feature = "stdlib_archive"))]
fn zip_archive_read_entry(_path: &str, _entry: &str) -> Result<Vec<u8>, std::io::Error> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "zip archive support requires the stdlib_archive feature",
    ))
}

#[cfg(not(feature = "stdlib_archive"))]
fn zip_archive_resources_path_payload(
    _archive_path: &str,
    _inner_path: &str,
    basename: String,
) -> ImportlibResourcesPathPayload {
    ImportlibResourcesPathPayload {
        basename,
        exists: false,
        is_file: false,
        is_dir: false,
        entries: Vec::new(),
        has_init_py: false,
        is_archive_member: true,
    }
}

#[cfg(not(feature = "stdlib_archive"))]
fn importlib_zip_source_exec_payload(
    _module_name: &str,
    _archive_path: &str,
    _inner_path: &str,
    _spec_is_package: bool,
) -> Result<ImportlibZipSourceExecPayload, std::io::Error> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "zip archive support requires the stdlib_archive feature",
    ))
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
    let mut zip_index_cache: HashMap<String, Option<ZipArchiveIndex>> = HashMap::new();
    for (idx, part) in parts.iter().enumerate() {
        let is_last = idx + 1 == parts.len();
        let mut found_pkg = false;
        let mut next_paths: Vec<String> = Vec::new();
        let mut namespace_paths: Vec<String> = Vec::new();
        let mut namespace_seen: HashSet<String> = HashSet::new();
        for base in &current_paths {
            if let Some((zip_archive, zip_prefix)) = split_zip_archive_path(base)
                && zip_archive_index_cached(&mut zip_index_cache, &zip_archive).is_some()
            {
                let pkg_rel = zip_entry_join(&zip_prefix, part);
                let init_entry = format!("{pkg_rel}/__init__.py");
                if zip_archive_entry_exists_cached(&mut zip_index_cache, &zip_archive, &init_entry)
                {
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
                if zip_archive_has_prefix_cached(&mut zip_index_cache, &zip_archive, &pkg_rel) {
                    append_unique_path_hashed(
                        &mut namespace_paths,
                        &mut namespace_seen,
                        &format!("{zip_archive}/{pkg_rel}"),
                    );
                }
                if is_last {
                    let mod_entry = zip_entry_join(&zip_prefix, &format!("{part}.py"));
                    if zip_archive_entry_exists_cached(
                        &mut zip_index_cache,
                        &zip_archive,
                        &mod_entry,
                    ) {
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
                append_unique_path_hashed(&mut namespace_paths, &mut namespace_seen, &pkg_dir);
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
    let mut seen: HashSet<String> = HashSet::new();
    for entry in search_paths {
        append_unique_path_hashed(&mut out, &mut seen, entry);
    }
    if let Some(stdlib_root) = state.stdlib_root.as_deref() {
        append_unique_path_hashed(&mut out, &mut seen, stdlib_root);
    }
    for root in &state.module_roots_entries {
        append_unique_path_hashed(&mut out, &mut seen, root);
    }
    for root in &state.venv_site_packages_entries {
        append_unique_path_hashed(&mut out, &mut seen, root);
    }
    for base in search_paths {
        let root = if base.is_empty() { "." } else { base.as_str() };
        let candidate =
            path_join_text(path_join_text(root.to_string(), "molt", sep), "stdlib", sep);
        append_unique_path_hashed(&mut out, &mut seen, &candidate);
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
    let mut resolved_seen: HashSet<String> = resolved.iter().cloned().collect();
    let state = sys_bootstrap_state_from_module_file(module_file);
    if !state.pwd.is_empty() {
        append_unique_path_hashed(&mut resolved, &mut resolved_seen, &state.pwd);
    }
    let mut matches: Vec<String> = Vec::new();
    let mut matches_seen: HashSet<String> = HashSet::new();
    let mut zip_index_cache: HashMap<String, Option<ZipArchiveIndex>> = HashMap::new();
    let parts: Vec<&str> = package.split('.').filter(|part| !part.is_empty()).collect();
    if parts.is_empty() {
        return matches;
    }
    for base in resolved {
        if let Some((archive_path, zip_prefix)) = split_zip_archive_path(&base)
            && zip_archive_index_cached(&mut zip_index_cache, &archive_path).is_some()
        {
            let mut rel = zip_prefix;
            for part in &parts {
                rel = zip_entry_join(&rel, part);
            }
            if zip_archive_has_prefix_cached(&mut zip_index_cache, &archive_path, &rel) {
                append_unique_path_hashed(
                    &mut matches,
                    &mut matches_seen,
                    &format!("{archive_path}/{rel}"),
                );
            }
            continue;
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
            append_unique_path_hashed(&mut matches, &mut matches_seen, &path);
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
    let mut seen: HashSet<String> = HashSet::new();
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
            append_unique_path_hashed(&mut out, &mut seen, &path_text);
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

fn importlib_enforce_extension_spec_boundary(
    _py: &PyToken<'_>,
    module_name: &str,
    resolution: &ImportlibPathResolution,
) -> Result<(), u64> {
    if resolution.loader_kind != "extension" {
        return Ok(());
    }
    let Some(origin) = resolution.origin.as_deref() else {
        return Err(raise_exception::<u64>(
            _py,
            "ImportError",
            "extension module path must point to a file",
        ));
    };
    match importlib_path_is_file(_py, origin) {
        Ok(true) => {}
        Ok(false) => {
            return Err(raise_exception::<u64>(
                _py,
                "ImportError",
                "extension module path must point to a file",
            ));
        }
        Err(bits) => return Err(bits),
    }
    importlib_require_extension_metadata(_py, module_name, origin)
}

fn importlib_find_spec_payload(
    _py: &PyToken<'_>,
    fullname: &str,
    search_paths: &[String],
    module_file: Option<String>,
    meta_path_count: i64,
    path_hooks_count: i64,
    package_context: bool,
) -> Result<Option<ImportlibFindSpecPayload>, u64> {
    let resolved = importlib_search_paths(search_paths, module_file);
    let Some(resolution) = importlib_find_in_path(fullname, &resolved, package_context) else {
        return Ok(None);
    };
    importlib_enforce_extension_spec_boundary(_py, fullname, &resolution)?;
    Ok(Some(ImportlibFindSpecPayload {
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
    }))
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
                if let Err(err) =
                    importlib_enforce_extension_spec_object_boundary(_py, fullname, spec_bits)
                {
                    dec_ref_bits(_py, spec_bits);
                    return Err(err);
                }
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
                if let Err(err) =
                    importlib_enforce_extension_spec_object_boundary(_py, fullname, spec_bits)
                {
                    dec_ref_bits(_py, spec_bits);
                    dec_ref_bits(_py, entry_bits);
                    for hook_bits in hooks {
                        dec_ref_bits(_py, hook_bits);
                    }
                    return Err(err);
                }
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
                        "invalid extension metadata in {archive_path}/extension_manifest.json: {err}"
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
        let allowed = has_capability(_py, cap);
        // Audit each individual capability check for extension loading.
        audit_emit(AuditEvent::new(
            "module.extension.cap_check",
            "module.extension",
            AuditArgs::Custom(cap.to_string()),
            if allowed {
                AuditDecision::Allowed
            } else {
                AuditDecision::Denied {
                    reason: format!("missing {cap} capability for extension {module_name:?}"),
                }
            },
            module_path!().to_string(),
        ));
        if !allowed {
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
            EXTENSION_METADATA_CACHE_HITS.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
    }

    EXTENSION_METADATA_CACHE_MISSES.fetch_add(1, Ordering::Relaxed);

    let extension_sha256 = importlib_sha256_path(_py, path)?;
    importlib_validate_extension_metadata(_py, module_name, path, &extension_sha256, &loaded)?;

    let cache = extension_metadata_ok_cache();
    let mut guard = cache
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.insert(cache_key, cache_value);
    Ok(())
}

fn importlib_path_has_extension_suffix(path: &str) -> bool {
    let lower = path.to_ascii_lowercase();
    lower.ends_with(".so")
        || lower.ends_with(".pyd")
        || lower.ends_with(".dll")
        || lower.ends_with(".dylib")
}

fn importlib_path_looks_like_extension(path: &str) -> bool {
    if let Some((_archive_path, inner_path)) = split_zip_archive_path(path) {
        return importlib_path_has_extension_suffix(&inner_path);
    }
    importlib_path_has_extension_suffix(path)
}

fn importlib_spec_attr_string(
    _py: &PyToken<'_>,
    target_bits: u64,
    name_slot: &AtomicU64,
    name: &'static [u8],
) -> Result<Option<String>, u64> {
    let attr_name = intern_static_name(_py, name_slot, name);
    let Some(attr_bits) = getattr_optional_bits(_py, target_bits, attr_name)? else {
        return Ok(None);
    };
    let value = string_obj_to_owned(obj_from_bits(attr_bits));
    if !obj_from_bits(attr_bits).is_none() {
        dec_ref_bits(_py, attr_bits);
    }
    Ok(value)
}

fn importlib_extension_spec_target(
    _py: &PyToken<'_>,
    expected_module_name: &str,
    spec_bits: u64,
) -> Result<Option<(String, String)>, u64> {
    static SPEC_NAME_NAME: AtomicU64 = AtomicU64::new(0);
    static SPEC_ORIGIN_NAME: AtomicU64 = AtomicU64::new(0);
    static SPEC_LOADER_NAME: AtomicU64 = AtomicU64::new(0);

    if obj_from_bits(spec_bits).is_none() {
        return Ok(None);
    }

    let module_name = importlib_spec_attr_string(_py, spec_bits, &SPEC_NAME_NAME, b"name")?
        .unwrap_or_else(|| expected_module_name.to_string());
    let origin = importlib_spec_attr_string(_py, spec_bits, &SPEC_ORIGIN_NAME, b"origin")?;
    let mut has_extension_loader = false;

    let loader_name = intern_static_name(_py, &SPEC_LOADER_NAME, b"loader");
    if let Some(loader_bits) = getattr_optional_bits(_py, spec_bits, loader_name)?
        && !obj_from_bits(loader_bits).is_none()
    {
        let loader_type = type_name(_py, obj_from_bits(loader_bits));
        has_extension_loader = loader_type.contains("ExtensionFileLoader");
        dec_ref_bits(_py, loader_bits);
    }

    let Some(path) = origin else {
        if has_extension_loader {
            return Err(raise_exception::<_>(
                _py,
                "ImportError",
                "extension module path must point to a file",
            ));
        }
        return Ok(None);
    };
    if !has_extension_loader && !importlib_path_looks_like_extension(&path) {
        return Ok(None);
    }
    Ok(Some((module_name, path)))
}

fn importlib_enforce_extension_spec_object_boundary(
    _py: &PyToken<'_>,
    expected_module_name: &str,
    spec_bits: u64,
) -> Result<(), u64> {
    let Some((module_name, path)) =
        importlib_extension_spec_target(_py, expected_module_name, spec_bits)?
    else {
        return Ok(());
    };

    if split_zip_archive_path(&path).is_none() {
        match importlib_path_is_file(_py, &path) {
            Ok(true) => {}
            Ok(false) => {
                return Err(raise_exception::<_>(
                    _py,
                    "ImportError",
                    "extension module path must point to a file",
                ));
            }
            Err(bits) => return Err(bits),
        }
    }
    importlib_require_extension_metadata(_py, &module_name, &path)
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

/// Load a native C extension (.so/.dylib/.pyd) via dlopen and inject its
/// module dict entries into the caller's namespace.
#[cfg(all(feature = "cext_loader", not(target_arch = "wasm32")))]
fn cext_loader_dlopen(
    _py: &PyToken<'_>,
    namespace_ptr: *mut u8,
    module_name: &str,
    path: &str,
) -> Result<(), String> {
    // Initialize the CPython ABI bridge and register runtime hooks (idempotent).
    molt_cpython_abi::bridge::molt_cpython_abi_init();
    crate::cpython_abi_hooks::register_cpython_hooks();

    // For "pkg.mod", use "mod" as the init function suffix.
    let init_name = module_name.rsplit('.').next().unwrap_or(module_name);

    let ext_path = std::path::Path::new(path);
    let module_bits =
        unsafe { molt_cpython_abi::loader::load_cpython_extension(ext_path, init_name) }
            .map_err(|e| format!("{e}"))?;

    if module_bits == 0 || module_bits == MoltObject::none().bits() {
        return Err("PyInit returned a null/None module".into());
    }

    // Copy the loaded module's __dict__ entries into the namespace dict.
    let module_obj = obj_from_bits(module_bits);
    if let Some(module_ptr) = module_obj.as_ptr() {
        let module_dict_bits = unsafe { crate::object::layout::module_dict_bits(module_ptr) };
        let module_dict = obj_from_bits(module_dict_bits);
        if let Some(dict_ptr) = module_dict.as_ptr()
            && unsafe { object_type_id(dict_ptr) == TYPE_ID_DICT }
        {
            unsafe {
                crate::object::ops_dict::dict_update_apply(
                    _py,
                    MoltObject::from_ptr(namespace_ptr).bits(),
                    crate::object::ops_dict::dict_update_set_in_place,
                    module_dict_bits,
                );
            }
        }
    }

    Ok(())
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
        if seen.insert(path.clone()) {
            out.push(path);
        }
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
    let mut out: Vec<String> = Vec::with_capacity(16);
    let mut seen: HashSet<String> = HashSet::new();
    append_unique_path_hashed(&mut out, &mut seen, &format!("{path}.molt.py"));
    append_unique_path_hashed(&mut out, &mut seen, &format!("{path}.py"));
    if let Some(stripped) = path.rsplit_once('.').map(|(prefix, _)| prefix) {
        append_unique_path_hashed(&mut out, &mut seen, &format!("{stripped}.molt.py"));
        append_unique_path_hashed(&mut out, &mut seen, &format!("{stripped}.py"));
        if let Some((prefix, _)) = stripped.rsplit_once(".cpython-") {
            append_unique_path_hashed(&mut out, &mut seen, &format!("{prefix}.molt.py"));
            append_unique_path_hashed(&mut out, &mut seen, &format!("{prefix}.py"));
        }
        if let Some((prefix, _)) = stripped.rsplit_once(".abi") {
            append_unique_path_hashed(&mut out, &mut seen, &format!("{prefix}.molt.py"));
            append_unique_path_hashed(&mut out, &mut seen, &format!("{prefix}.py"));
        }
        if let Some((prefix, _)) = stripped.rsplit_once(".cp") {
            append_unique_path_hashed(&mut out, &mut seen, &format!("{prefix}.molt.py"));
            append_unique_path_hashed(&mut out, &mut seen, &format!("{prefix}.py"));
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
            append_unique_path_hashed(&mut out, &mut seen, &molt_candidate);
            let py_candidate = path_join_text(parent, &format!("{module_stem}.py"), sep);
            append_unique_path_hashed(&mut out, &mut seen, &py_candidate);
        }
    }
    let dirname = path_dirname_text(path, sep);
    let local_name = module_name.rsplit('.').next().unwrap_or(module_name);
    if !local_name.is_empty() {
        let named_molt = path_join_text(dirname.clone(), &format!("{local_name}.molt.py"), sep);
        append_unique_path_hashed(&mut out, &mut seen, &named_molt);
        let named_py = path_join_text(dirname, &format!("{local_name}.py"), sep);
        append_unique_path_hashed(&mut out, &mut seen, &named_py);
        let package_dir = path_join_text(path_dirname_text(path, sep), local_name, sep);
        let pkg_init_molt = path_join_text(package_dir.clone(), "__init__.molt.py", sep);
        append_unique_path_hashed(&mut out, &mut seen, &pkg_init_molt);
        let pkg_init_py = path_join_text(package_dir, "__init__.py", sep);
        append_unique_path_hashed(&mut out, &mut seen, &pkg_init_py);
    }
    let basename = path_basename_text(path, sep);
    if basename.starts_with("__init__.") {
        append_unique_path_hashed(
            &mut out,
            &mut seen,
            &path_join_text(path_dirname_text(path, sep), "__init__.molt.py", sep),
        );
        append_unique_path_hashed(
            &mut out,
            &mut seen,
            &path_join_text(path_dirname_text(path, sep), "__init__.py", sep),
        );
    }
    out
}

fn importlib_sourceless_source_candidates(module_name: &str, path: &str) -> Vec<String> {
    let sep = bootstrap_path_sep();
    let mut out: Vec<String> = Vec::with_capacity(12);
    let mut seen: HashSet<String> = HashSet::new();
    if let Some(stripped) = path.strip_suffix(".pyc") {
        append_unique_path_hashed(&mut out, &mut seen, &format!("{stripped}.molt.py"));
        append_unique_path_hashed(&mut out, &mut seen, &format!("{stripped}.py"));
        if let Some((prefix, _)) = stripped.rsplit_once(".cpython-") {
            append_unique_path_hashed(&mut out, &mut seen, &format!("{prefix}.molt.py"));
            append_unique_path_hashed(&mut out, &mut seen, &format!("{prefix}.py"));
        }
        if let Some((prefix, _)) = stripped.rsplit_once(".pypy-") {
            append_unique_path_hashed(&mut out, &mut seen, &format!("{prefix}.molt.py"));
            append_unique_path_hashed(&mut out, &mut seen, &format!("{prefix}.py"));
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
            append_unique_path_hashed(&mut out, &mut seen, &molt_candidate);
            let candidate = path_join_text(parent, &format!("{module_name}.py"), sep);
            append_unique_path_hashed(&mut out, &mut seen, &candidate);
        }
    }
    let dirname = path_dirname_text(path, sep);
    let local_name = module_name.rsplit('.').next().unwrap_or(module_name);
    if !local_name.is_empty() {
        let named_molt = path_join_text(dirname.clone(), &format!("{local_name}.molt.py"), sep);
        append_unique_path_hashed(&mut out, &mut seen, &named_molt);
        let named_py = path_join_text(dirname, &format!("{local_name}.py"), sep);
        append_unique_path_hashed(&mut out, &mut seen, &named_py);
    }
    let basename = path_basename_text(path, sep);
    if basename.starts_with("__init__.") && basename.ends_with(".pyc") {
        append_unique_path_hashed(
            &mut out,
            &mut seen,
            &path_join_text(path_dirname_text(path, sep), "__init__.molt.py", sep),
        );
        append_unique_path_hashed(
            &mut out,
            &mut seen,
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

fn importlib_target_minor() -> i64 {
    // Check explicit env-var overrides only.  Do NOT read from
    // state.sys_version_info — that field is tainted by the host Python
    // version used to run the compiler.  Default to 12 (molt's target).
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
    let target_minor = importlib_target_minor();
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

fn importlib_module_has_key(
    _py: &PyToken<'_>,
    module_bits: u64,
    name_slot: &AtomicU64,
    name: &'static [u8],
) -> Result<bool, u64> {
    let Some(dict_ptr) = importlib_module_dict_ptr(module_bits) else {
        return Ok(false);
    };
    let key_bits = intern_static_name(_py, name_slot, name);
    Ok(importlib_dict_get_string_key_bits(_py, dict_ptr, key_bits)?.is_some())
}

fn importlib_module_is_intrinsic_shell(
    _py: &PyToken<'_>,
    module_name: &str,
    module_bits: u64,
) -> Result<bool, u64> {
    static INTRINSIC_LOOKUP_NAME: AtomicU64 = AtomicU64::new(0);
    static INTRINSICS_NAME: AtomicU64 = AtomicU64::new(0);
    static RUNTIME_NAME: AtomicU64 = AtomicU64::new(0);

    if !importlib_module_public_surface_empty(_py, module_name, module_bits)? {
        return Ok(false);
    }
    Ok(importlib_module_has_key(
        _py,
        module_bits,
        &INTRINSIC_LOOKUP_NAME,
        b"_molt_intrinsic_lookup",
    )? || importlib_module_has_key(_py, module_bits, &INTRINSICS_NAME, b"_molt_intrinsics")?
        || importlib_module_has_key(_py, module_bits, &RUNTIME_NAME, b"_molt_runtime")?)
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
    if importlib_module_is_intrinsic_shell(_py, module_name, module_bits)? {
        return Ok(true);
    }
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
    if let Err(err) = importlib_enforce_extension_spec_object_boundary(_py, resolved, spec_bits) {
        if !obj_from_bits(spec_bits).is_none() {
            dec_ref_bits(_py, spec_bits);
        }
        return Err(err);
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
    let result = importlib_import_with_fallback_inner(
        _py,
        resolved,
        resolved_bits,
        modules_ptr,
        util_bits,
        machinery_bits,
    );

    // If every import mechanism failed with ModuleNotFoundError, try loading a
    // native C extension (.so / .dylib) from sys.path before giving up.
    #[cfg(all(feature = "cext_loader", not(target_arch = "wasm32")))]
    if result.is_err() && importlib_exception_should_fallback(_py) {
        if let Some(module_bits) = importlib_try_cext_on_sys_path(_py, resolved, modules_ptr) {
            return Ok(module_bits);
        }
        // Extension search failed too – restore the original error.
        return Err(raise_exception::<_>(
            _py,
            "ModuleNotFoundError",
            &format!("No module named '{resolved}'"),
        ));
    }

    result
}

fn importlib_import_with_fallback_inner(
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

/// Scan `sys.path` directories for a native C extension matching `module_name`,
/// load it via dlopen, register it in sys.modules, and return its module bits.
#[cfg(all(feature = "cext_loader", not(target_arch = "wasm32")))]
fn importlib_try_cext_on_sys_path(
    _py: &PyToken<'_>,
    module_name: &str,
    modules_ptr: *mut u8,
) -> Option<u64> {
    // Retrieve sys.path as a Vec<String>.
    static PATH_NAME_CEXT: AtomicU64 = AtomicU64::new(0);
    let sys_bits = importlib_sys_module_bits(_py)?;
    if obj_from_bits(sys_bits).is_none() {
        return None;
    }
    let path_name = intern_static_name(_py, &PATH_NAME_CEXT, b"path");
    let path_attr = getattr_optional_bits(_py, sys_bits, path_name).ok()??;
    let search_paths = string_sequence_arg_from_bits(_py, path_attr, "sys.path").ok()?;
    if !obj_from_bits(path_attr).is_none() {
        dec_ref_bits(_py, path_attr);
    }

    // Search each directory for a matching .so / .dylib file.
    for dir in &search_paths {
        if let Some(ext_path) = importlib_find_extension_module(dir, module_name) {
            // Found a candidate – attempt dlopen.
            molt_cpython_abi::bridge::molt_cpython_abi_init();
            crate::cpython_abi_hooks::register_cpython_hooks();

            let init_name = module_name.rsplit('.').next().unwrap_or(module_name);
            let path_obj = std::path::Path::new(&ext_path);
            let module_bits = match unsafe {
                molt_cpython_abi::loader::load_cpython_extension(path_obj, init_name)
            } {
                Ok(bits) => bits,
                Err(_) => continue,
            };
            if module_bits == 0 || module_bits == MoltObject::none().bits() {
                continue;
            }

            // Register in sys.modules so subsequent imports hit the cache.
            if let Ok(key_bits) = alloc_str_bits(_py, module_name) {
                let _ = importlib_dict_set_string_key(_py, modules_ptr, key_bits, module_bits);
                dec_ref_bits(_py, key_bits);
            }

            return Some(module_bits);
        }
    }
    None
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

#[path = "platform_importlib_ffi.rs"]
mod importlib_ffi;
pub use importlib_ffi::*;

#[path = "platform_env_ffi.rs"]
mod env_ffi;
pub use env_ffi::*;

#[cfg(test)]
#[path = "platform_tests.rs"]
mod tests;

#[cfg(all(target_arch = "wasm32", not(feature = "wasm_freestanding")))]
#[link(wasm_import_module = "wasi_snapshot_preview1")]
unsafe extern "C" {
    fn environ_sizes_get(environ_count: *mut u32, environ_buf_size: *mut u32) -> u16;
    fn environ_get(environ: *mut *mut u8, environ_buf: *mut u8) -> u16;
}
