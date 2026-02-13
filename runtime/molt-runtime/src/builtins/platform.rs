use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;
use std::sync::atomic::AtomicU64;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use digest::Digest;
use getrandom::fill as getrandom_fill;
use md5::Md5;
use sha1::Sha1;

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
        if best_idx.map_or(true, |current| idx > current) {
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
    let found = archive.by_name(entry).is_ok();
    found
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
            if let Some(child) = rel.split('/').next() {
                if !child.is_empty() {
                    entries.insert(child.to_string());
                }
            }
            continue;
        }

        if let Some(child) = name.split('/').next() {
            if !child.is_empty() {
                entries.insert(child.to_string());
            }
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
    package_context: bool,
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
            zip_archive: None,
            zip_inner_path: None,
            meta_path_count,
            path_hooks_count,
        });
    }
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
            if let Some(cache_ptr) = path_importer_cache_ptr {
                if let Some(cached_bits) = unsafe { dict_get_in_place(_py, cache_ptr, entry_bits) }
                {
                    if obj_from_bits(cached_bits).is_none() {
                        dec_ref_bits(_py, entry_bits);
                        continue;
                    }
                    finder_bits = cached_bits;
                }
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

    if let Some(sys_bits) = sys_bits {
        if !obj_from_bits(sys_bits).is_none() {
            modules_bits =
                importlib_runtime_state_attr_bits(_py, sys_bits, &MODULES_NAME, b"modules")?;
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
    }

    let keys_and_values: [(&[u8], u64); 4] = [
        (b"modules", modules_bits),
        (b"meta_path", meta_path_bits),
        (b"path_hooks", path_hooks_bits),
        (b"path_importer_cache", path_importer_cache_bits),
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
        if !obj_from_bits(bits).is_none() {
            dec_ref_bits(_py, bits);
        }
    }
    if dict_ptr.is_null() {
        Err(raise_exception::<_>(_py, "MemoryError", "out of memory"))
    } else {
        Ok(MoltObject::from_ptr(dict_ptr).bits())
    }
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
        } else if spec.is_package && spec.loader_kind == "namespace" {
            if let Some(locations) = spec.submodule_search_locations {
                for location in locations {
                    append_unique_path(&mut roots, &location);
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
    // TODO(stdlib-compat, owner:runtime, milestone:SL3, priority:P1, status:partial): replace restricted source shim execution with native extension and pyc execution parity.
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
        if let Some(name) = out {
            if !name.is_empty() {
                return Ok(name);
            }
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
            if let Some(name) = out {
                if !name.is_empty() {
                    return Ok(name);
                }
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
        if let Some(name) = out {
            if !name.is_empty() {
                return Ok(name);
            }
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
        if !out.iter().any(|value| value == &entry) {
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
        if !out.iter().any(|value| value == &name) {
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
    {
        if payload.is_dir {
            return Ok(payload.entries);
        }
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

    let parts = vec![name.to_string()];
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
    {
        if payload.is_file && !payload.is_archive_member {
            return Ok(Some(joined));
        }
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
    {
        if payload.is_dir {
            return Ok(payload.entries);
        }
    }
    let entries = importlib_resources_reader_contents_impl(_py, reader_bits)?;
    let prefix = parts.join("/");
    let mut out: Vec<String> = Vec::new();
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
        if !out.iter().any(|name| name == child) {
            out.push(child.to_string());
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
    let parts = vec![name.to_string()];
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
                let parts = vec![name.to_string()];
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

    let parts = vec![name.to_string()];
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
        let module_name = match string_arg_from_bits(_py, module_name_bits, "module name") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        let path = match string_arg_from_bits(_py, path_bits, "path") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
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
pub extern "C" fn molt_traceback_exception_suppress_context(value_bits: u64) -> u64 {
    crate::with_gil_entry!(_py, {
        match traceback_exception_suppress_context_bits(_py, value_bits) {
            Ok(value) => MoltObject::from_bool(value).bits(),
            Err(bits) => bits,
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
            if value < 0 || value > 0xFFFF_FFFF_FFFF_i64 {
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
            if value < 0 || value > 0x3FFF_i64 {
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
        return vec![
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
        ];
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
                    importlib_search_paths(&vec!["src".to_string()], Some(bootstrap_module_file()));
                assert!(resolved.iter().any(|entry| entry == "src"));
                assert!(resolved.iter().any(|entry| entry == "vendor"));
                assert!(resolved.iter().any(|entry| entry == "extra"));
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
        let resolved = importlib_namespace_paths(
            "nszip.pkg",
            &vec![archive_text],
            Some(bootstrap_module_file()),
        );
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
        assert!(!payload.is_archive_member);
        assert!(payload.entries.iter().any(|entry| entry == "__init__.py"));
        assert!(payload.entries.iter().any(|entry| entry == "data.txt"));

        std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
    }

    #[test]
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
            &vec![archive_text.clone()],
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
                assert_eq!(payload.pythonpath_entries, vec!["alpha".to_string()]);
                assert!(
                    payload
                        .module_roots_entries
                        .iter()
                        .any(|entry| entry == "vendor")
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
