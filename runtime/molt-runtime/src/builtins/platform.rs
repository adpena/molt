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
use crate::builtins::exceptions::molt_exception_last_pending;
use crate::builtins::io::{
    path_basename_text, path_dirname_text, path_join_text, path_normpath_text,
};
use crate::builtins::modules::{runpy_exec_restricted_source, sys_modules_dict_bits};
#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
use crate::object::ops_sys::runtime_target_minor;
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
    crate::with_gil_entry_nopanic!(_py, {
        let msg = format_obj_str(_py, obj_from_bits(msg_bits));
        eprintln!("Molt bridge unavailable: {msg}");
        std::process::exit(1);
    })
}

const PLATFORM_OBJECT_SLOT_COUNT: usize = 4;

pub(crate) struct PlatformRuntimeState {
    errno_constants_cache: AtomicU64,
    socket_constants_cache: AtomicU64,
    os_name_cache: AtomicU64,
    sys_platform_cache: AtomicU64,
}

impl PlatformRuntimeState {
    pub(crate) fn new() -> Self {
        Self {
            errno_constants_cache: AtomicU64::new(0),
            socket_constants_cache: AtomicU64::new(0),
            os_name_cache: AtomicU64::new(0),
            sys_platform_cache: AtomicU64::new(0),
        }
    }

    fn object_slots(&self) -> [&AtomicU64; PLATFORM_OBJECT_SLOT_COUNT] {
        [
            &self.errno_constants_cache,
            &self.socket_constants_cache,
            &self.os_name_cache,
            &self.sys_platform_cache,
        ]
    }
}

fn platform_state(_py: &PyToken<'_>) -> &'static PlatformRuntimeState {
    &runtime_state(_py).platform
}

fn init_platform_cached_owned_bits(
    _py: &PyToken<'_>,
    slot: &AtomicU64,
    init: impl FnOnce() -> u64,
) -> u64 {
    let bits = init_atomic_bits(_py, slot, init);
    inc_ref_bits(_py, bits);
    bits
}

pub(crate) fn platform_clear_runtime_state(_py: &PyToken<'_>, state: &crate::state::RuntimeState) {
    crate::gil_assert();
    let slots = state.platform.object_slots();
    crate::state::cache::clear_atomic_slots(_py, &slots);
}

static ENV_STATE: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
static PROCESS_ENV_STATE: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
static LOCALE_STATE: OnceLock<Mutex<String>> = OnceLock::new();
static UUID_NODE_STATE: OnceLock<Mutex<Option<u64>>> = OnceLock::new();
static UUID_V1_STATE: OnceLock<Mutex<(Option<u16>, u64)>> = OnceLock::new();
static EXTENSION_METADATA_OK_CACHE: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
#[cfg(all(feature = "source_extension_loader", not(target_arch = "wasm32")))]
static SOURCE_EXTENSION_LIBRARIES: OnceLock<Mutex<Vec<libloading::Library>>> = OnceLock::new();
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

fn bootstrap_stdlib_root_from_module_file(module_file: &str, path_sep: char) -> Option<String> {
    if module_file.is_empty() {
        return None;
    }
    let module_dir = path_dirname_text(module_file, path_sep);
    if module_dir.is_empty() {
        return None;
    }
    let mut current = module_dir.clone();
    loop {
        if path_basename_text(&current, path_sep) == "stdlib" {
            let parent = path_dirname_text(&current, path_sep);
            if path_basename_text(&parent, path_sep) == "molt" {
                return Some(current);
            }
        }
        let parent = path_dirname_text(&current, path_sep);
        if parent.is_empty() || parent == current {
            break;
        }
        current = parent;
    }
    Some(module_dir)
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
    let (module_roots_raw, dev_trusted_raw, pwd_raw, windows_paths) = {
        let guard = env_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        (
            guard.get("MOLT_MODULE_ROOTS").cloned().unwrap_or_default(),
            guard.get("MOLT_DEV_TRUSTED").cloned().unwrap_or_default(),
            guard.get("PWD").cloned().unwrap_or_default(),
            sys_platform_str().starts_with("win"),
        )
    };

    let sep = if windows_paths { ';' } else { ':' };
    let path_sep = if windows_paths { '\\' } else { '/' };
    let pwd = resolve_bootstrap_pwd(&pwd_raw);
    let pythonpath_entries: Vec<String> = Vec::new();
    let py_path_raw = String::new();
    let mut paths: Vec<String> = pythonpath_entries.clone();
    let mut paths_seen: HashSet<String> = pythonpath_entries.iter().cloned().collect();

    let stdlib_root = module_file.and_then(|path| {
        bootstrap_stdlib_root_from_module_file(&path, path_sep)
            .map(|root| bootstrap_resolve_path_entry(&root, &pwd, path_sep))
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

    let venv_site_packages_entries: Vec<String> = Vec::new();
    let virtual_env_raw = String::new();

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
        let find_spec_name = intern_runtime_static_name(_py, b"find_spec");
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
        let find_spec_name = intern_runtime_static_name(_py, b"find_spec");
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
        modules_bits = importlib_runtime_state_attr_bits(
            _py,
            sys_bits,
            runtime_static_name_slot(_py, b"modules"),
            b"modules",
        )?;
        meta_path_bits = importlib_runtime_state_attr_bits(
            _py,
            sys_bits,
            runtime_static_name_slot(_py, b"meta_path"),
            b"meta_path",
        )?;
        path_hooks_bits = importlib_runtime_state_attr_bits(
            _py,
            sys_bits,
            runtime_static_name_slot(_py, b"path_hooks"),
            b"path_hooks",
        )?;
        path_importer_cache_bits = importlib_runtime_state_attr_bits(
            _py,
            sys_bits,
            runtime_static_name_slot(_py, b"path_importer_cache"),
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
        (intern_runtime_static_name(_py, b"modules"), modules_bits),
        (
            intern_runtime_static_name(_py, b"meta_path"),
            meta_path_bits,
        ),
        (
            intern_runtime_static_name(_py, b"path_hooks"),
            path_hooks_bits,
        ),
        (
            intern_runtime_static_name(_py, b"path_importer_cache"),
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

#[cfg(all(feature = "source_extension_loader", not(target_arch = "wasm32")))]
fn source_extension_libraries() -> &'static Mutex<Vec<libloading::Library>> {
    SOURCE_EXTENSION_LIBRARIES.get_or_init(|| Mutex::new(Vec::new()))
}

#[cfg(all(feature = "source_extension_loader", not(target_arch = "wasm32")))]
fn source_extension_merge_module_dict(
    _py: &PyToken<'_>,
    namespace_ptr: *mut u8,
    module_bits: u64,
) -> Result<(), String> {
    let Some(module_ptr) = obj_from_bits(module_bits).as_ptr() else {
        return Err("PyInit returned an invalid module handle".to_string());
    };
    if unsafe { object_type_id(module_ptr) } != TYPE_ID_MODULE {
        return Err("PyInit returned a non-module object".to_string());
    }
    let module_dict_bits = unsafe { crate::object::layout::module_dict_bits(module_ptr) };
    let Some(dict_ptr) = obj_from_bits(module_dict_bits).as_ptr() else {
        return Err("PyInit module has no dictionary".to_string());
    };
    if unsafe { object_type_id(dict_ptr) } != TYPE_ID_DICT {
        return Err("PyInit module dictionary is not a dict".to_string());
    }
    unsafe {
        crate::object::ops_dict::dict_update_apply(
            _py,
            MoltObject::from_ptr(namespace_ptr).bits(),
            crate::object::ops_dict::dict_update_set_in_place,
            module_dict_bits,
        );
    }
    if exception_pending(_py) {
        clear_exception(_py);
        return Err("module dictionary merge raised".to_string());
    }
    Ok(())
}

#[cfg(all(feature = "source_extension_loader", not(target_arch = "wasm32")))]
fn source_extension_install_sys_module(
    _py: &PyToken<'_>,
    module_name: &str,
    module_bits: u64,
) -> Result<(), String> {
    let modules_bits = match importlib_runtime_modules_bits(_py) {
        Ok(bits) => bits,
        Err(_) => {
            if exception_pending(_py) {
                clear_exception(_py);
            }
            return Err("failed to access sys.modules".to_string());
        }
    };
    let Some(modules_ptr) = obj_from_bits(modules_bits).as_ptr() else {
        return Err("sys.modules is not a dict".to_string());
    };
    if unsafe { object_type_id(modules_ptr) } != TYPE_ID_DICT {
        return Err("sys.modules is not a dict".to_string());
    }
    let name_bits = match alloc_str_bits(_py, module_name) {
        Ok(bits) => bits,
        Err(_) => {
            if exception_pending(_py) {
                clear_exception(_py);
            }
            return Err("failed to allocate sys.modules key".to_string());
        }
    };
    let set_result = importlib_dict_set_string_key(_py, modules_ptr, name_bits, module_bits);
    if !obj_from_bits(name_bits).is_none() {
        dec_ref_bits(_py, name_bits);
    }
    if set_result.is_err() {
        if exception_pending(_py) {
            clear_exception(_py);
        }
        return Err("failed to install source extension in sys.modules".to_string());
    }
    Ok(())
}

#[cfg(all(feature = "source_extension_loader", not(target_arch = "wasm32")))]
fn source_extension_loader_dlopen(
    _py: &PyToken<'_>,
    namespace_ptr: *mut u8,
    module_name: &str,
    path: &str,
    init_symbol: &str,
) -> Result<(), String> {
    type InitFn = unsafe extern "C" fn() -> *mut u8;
    let library = unsafe { libloading::Library::new(Path::new(path)) }
        .map_err(|err| format!("dlopen failed: {err}"))?;
    let module_ptr = {
        let init_fn: libloading::Symbol<'_, InitFn> = unsafe {
            library
                .get(init_symbol.as_bytes())
                .map_err(|err| format!("missing init symbol {init_symbol}: {err}"))?
        };
        unsafe { init_fn() }
    };
    if exception_pending(_py) {
        clear_exception(_py);
        return Err(format!("{init_symbol} raised during initialization"));
    }
    if module_ptr.is_null() {
        return Err(format!("{init_symbol} returned NULL"));
    }
    let module_bits = module_ptr as usize as u64;
    source_extension_merge_module_dict(_py, namespace_ptr, module_bits)?;
    source_extension_install_sys_module(_py, module_name, module_bits)?;
    let mut guard = source_extension_libraries()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.push(library);
    Ok(())
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
    let get_source_name = intern_runtime_static_name(_py, b"get_source");
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
    let spec_name = intern_runtime_static_name(_py, b"__spec__");
    let Some(spec_bits) = getattr_optional_bits(_py, module_bits, spec_name)? else {
        return Ok(false);
    };
    let submodule_search_locations_name = intern_static_name(
        _py,
        runtime_static_name_slot(_py, b"submodule_search_locations"),
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
    if obj_from_bits(value_bits).is_none() {
        return Ok(false);
    }
    let suppress_context_name = intern_runtime_static_name(_py, b"__suppress_context__");
    let Some(suppress_bits) = getattr_optional_bits(_py, value_bits, suppress_context_name)? else {
        return Ok(false);
    };
    let out = is_truthy(_py, obj_from_bits(suppress_bits));
    if !obj_from_bits(suppress_bits).is_none() {
        dec_ref_bits(_py, suppress_bits);
    }
    Ok(out)
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

pub(crate) fn known_absent_module_missing_name(
    _py: &PyToken<'_>,
    resolved: &str,
) -> Option<String> {
    let target_minor = runtime_target_minor(_py);
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
    let cached_sys_bits = {
        let cache = crate::builtins::exceptions::internals::module_cache(_py);
        let guard = cache.lock().unwrap();
        guard.get("sys").copied()
    };
    let sys_bits = if let Some(sys_bits) = cached_sys_bits {
        inc_ref_bits(_py, sys_bits);
        sys_bits
    } else {
        let sys_name_bits = alloc_str_bits(_py, "sys")?;
        let imported_bits = crate::molt_module_import(sys_name_bits);
        dec_ref_bits(_py, sys_name_bits);
        if exception_pending(_py) {
            if !obj_from_bits(imported_bits).is_none() {
                dec_ref_bits(_py, imported_bits);
            }
            return Err(MoltObject::none().bits());
        }
        if obj_from_bits(imported_bits).is_none() {
            return Err(importlib_modules_runtime_error(_py));
        }
        imported_bits
    };
    if obj_from_bits(sys_bits).is_none() {
        dec_ref_bits(_py, sys_bits);
        return Err(importlib_modules_runtime_error(_py));
    }
    let Some(modules_bits) = sys_modules_dict_bits(_py, sys_bits) else {
        dec_ref_bits(_py, sys_bits);
        return Err(importlib_modules_runtime_error(_py));
    };
    dec_ref_bits(_py, sys_bits);
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
    let saved_exc_bits = if exception_pending(_py) {
        let bits = molt_exception_last_pending();
        clear_exception(_py);
        Some(bits)
    } else {
        None
    };
    unsafe {
        let _ = dict_del_in_place(_py, dict_ptr, key_bits);
    }
    if let Some(bits) = saved_exc_bits {
        if exception_pending(_py) {
            clear_exception(_py);
        }
        if !obj_from_bits(bits).is_none() {
            let _ = crate::molt_exception_set_last(bits);
        }
        dec_ref_bits(_py, bits);
    } else if exception_pending(_py) {
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

    let spec_name = intern_runtime_static_name(_py, b"__spec__");
    if let Some(spec_bits) = getattr_optional_bits(_py, existing_bits, spec_name)?
        && !obj_from_bits(spec_bits).is_none()
    {
        if !obj_from_bits(module_name_key_bits).is_none() {
            dec_ref_bits(_py, module_name_key_bits);
        }
        return Ok(spec_bits);
    }

    let file_name = intern_runtime_static_name(_py, b"__file__");
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
        runtime_static_name_slot(_py, b"ModuleSpec"),
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
        let path_name = intern_runtime_static_name(_py, b"__path__");
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

        let modules_key = intern_runtime_static_name(_py, b"modules");
        let meta_path_key = intern_runtime_static_name(_py, b"meta_path");
        let path_hooks_key = intern_runtime_static_name(_py, b"path_hooks");
        let path_importer_cache_key = intern_runtime_static_name(_py, b"path_importer_cache");
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
    let clear_name = intern_runtime_static_name(_py, b"clear");
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
    let name_attr = intern_runtime_static_name(_py, b"__name__");
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
    if !importlib_module_public_surface_empty(_py, module_name, module_bits)? {
        return Ok(false);
    }
    Ok(importlib_module_has_key(
        _py,
        module_bits,
        runtime_static_name_slot(_py, b"_molt_intrinsic_lookup"),
        b"_molt_intrinsic_lookup",
    )? || importlib_module_has_key(
        _py,
        module_bits,
        runtime_static_name_slot(_py, b"_molt_intrinsics"),
        b"_molt_intrinsics",
    )? || importlib_module_has_key(
        _py,
        module_bits,
        runtime_static_name_slot(_py, b"_molt_runtime"),
        b"_molt_runtime",
    )?)
}

fn importlib_module_is_empty_placeholder(
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

    let spec_name = intern_runtime_static_name(_py, b"__spec__");
    let file_name = intern_runtime_static_name(_py, b"__file__");
    let loader_name = intern_runtime_static_name(_py, b"loader");
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
    let exc_bits = molt_exception_last_pending();
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

fn missing_module_name_from_message(message: &str) -> Option<&str> {
    for (prefix, quote) in [("No module named '", '\''), ("No module named \"", '"')] {
        let Some(rest) = message.strip_prefix(prefix) else {
            continue;
        };
        let end = rest.find(quote)?;
        return Some(&rest[..end]);
    }
    message
        .strip_prefix("No module named ")
        .and_then(|rest| rest.split_whitespace().next())
        .filter(|name| !name.is_empty())
}

fn missing_module_matches_import(missing: &str, resolved: &str) -> bool {
    missing == resolved
        || resolved
            .strip_prefix(missing)
            .is_some_and(|suffix| suffix.starts_with('.'))
}

fn importlib_rethrow_pending_exception(_py: &PyToken<'_>) {
    let Some((kind, message)) = pending_exception_kind_and_message(_py) else {
        return;
    };
    clear_exception(_py);
    let _ = raise_exception::<u64>(_py, &kind, &message);
}

fn importlib_exception_should_fallback(_py: &PyToken<'_>, resolved: &str) -> bool {
    let Some((kind, message)) = pending_exception_kind_and_message(_py) else {
        return false;
    };
    if kind == "ImportError" || kind == "ModuleNotFoundError" {
        if missing_module_name_from_message(&message)
            .is_some_and(|missing| missing_module_matches_import(missing, resolved))
        {
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
    let attr_name = intern_runtime_static_name(_py, b"BuiltinImporter");
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
    let cache_name = intern_runtime_static_name(_py, b"_MOLT_LOADER");
    if let Some(loader_bits) = getattr_optional_bits(_py, machinery_bits, cache_name)?
        && !obj_from_bits(loader_bits).is_none()
    {
        return Ok(loader_bits);
    }
    let loader_bits = importlib_machinery_loader_instance(
        _py,
        machinery_bits,
        runtime_static_name_slot(_py, b"BuiltinImporter"),
        b"BuiltinImporter",
        &[],
    )?;
    if let Err(err) = importlib_set_attr(
        _py,
        machinery_bits,
        runtime_static_name_slot(_py, b"_MOLT_LOADER"),
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
            runtime_static_name_slot(_py, b"SourceFileLoader"),
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
            runtime_static_name_slot(_py, b"ExtensionFileLoader"),
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
            runtime_static_name_slot(_py, b"SourcelessFileLoader"),
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
                runtime_static_name_slot(_py, b"_ZipSourceLoader"),
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
        runtime_static_name_slot(_py, b"ModuleSpec"),
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
            runtime_static_name_slot(_py, b"submodule_search_locations"),
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
    if let Err(err) = importlib_set_attr(
        _py,
        spec_bits,
        runtime_static_name_slot(_py, b"cached"),
        b"cached",
        cached_bits,
    ) {
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
        runtime_static_name_slot(_py, b"has_location"),
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
    importlib_set_attr(
        _py,
        module_bits,
        runtime_static_name_slot(_py, b"__loader__"),
        b"__loader__",
        loader_bits,
    )?;
    importlib_set_attr(
        _py,
        module_bits,
        runtime_static_name_slot(_py, b"__file__"),
        b"__file__",
        origin_bits,
    )?;
    importlib_set_attr(
        _py,
        module_bits,
        runtime_static_name_slot(_py, b"__cached__"),
        b"__cached__",
        MoltObject::none().bits(),
    )?;
    importlib_set_attr(
        _py,
        module_bits,
        runtime_static_name_slot(_py, b"__package__"),
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
    importlib_set_attr(
        _py,
        spec_bits,
        runtime_static_name_slot(_py, b"loader"),
        b"loader",
        loader_bits,
    )?;
    importlib_set_attr(
        _py,
        spec_bits,
        runtime_static_name_slot(_py, b"origin"),
        b"origin",
        origin_bits,
    )?;
    importlib_set_attr(
        _py,
        spec_bits,
        runtime_static_name_slot(_py, b"has_location"),
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
    let cached_name = intern_runtime_static_name(_py, b"cached");
    let origin_name = intern_runtime_static_name(_py, b"origin");
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
    let out = importlib_set_attr(
        _py,
        spec_bits,
        runtime_static_name_slot(_py, b"cached"),
        b"cached",
        cached_bits,
    );
    if !obj_from_bits(cached_bits).is_none() {
        dec_ref_bits(_py, cached_bits);
    }
    out
}

fn importlib_module_support_bits(
    _py: &PyToken<'_>,
    modules_ptr: *mut u8,
    module_name: &'static str,
) -> Result<u64, u64> {
    let key_bits = alloc_str_bits(_py, module_name)?;
    let cached_bits = unsafe { dict_get_in_place(_py, modules_ptr, key_bits) };
    if exception_pending(_py) {
        dec_ref_bits(_py, key_bits);
        return Err(MoltObject::none().bits());
    }
    if let Some(cached_bits) = cached_bits {
        dec_ref_bits(_py, key_bits);
        if obj_from_bits(cached_bits).is_none() {
            return Err(raise_exception::<_>(
                _py,
                "ImportError",
                &format!("import of {module_name} halted; None in sys.modules"),
            ));
        }
        inc_ref_bits(_py, cached_bits);
        return Ok(cached_bits);
    }

    let imported_bits = crate::molt_module_import(key_bits);
    dec_ref_bits(_py, key_bits);
    if exception_pending(_py) {
        if !obj_from_bits(imported_bits).is_none() {
            dec_ref_bits(_py, imported_bits);
        }
        return Err(MoltObject::none().bits());
    }
    if obj_from_bits(imported_bits).is_none() {
        return Err(raise_exception::<_>(
            _py,
            "RuntimeError",
            &format!("runtime support module unavailable: {module_name}"),
        ));
    }
    Ok(imported_bits)
}

fn importlib_import_via_spec(
    _py: &PyToken<'_>,
    resolved: &str,
    resolved_bits: u64,
    modules_ptr: *mut u8,
) -> Result<u64, u64> {
    let util_bits = importlib_module_support_bits(_py, modules_ptr, "importlib.util")?;
    let machinery_bits =
        match importlib_module_support_bits(_py, modules_ptr, "importlib.machinery") {
            Ok(bits) => bits,
            Err(err) => {
                if !obj_from_bits(util_bits).is_none() {
                    dec_ref_bits(_py, util_bits);
                }
                return Err(err);
            }
        };
    let out = importlib_import_via_spec_with_support(
        _py,
        resolved,
        resolved_bits,
        modules_ptr,
        util_bits,
        machinery_bits,
    );
    if !obj_from_bits(util_bits).is_none() {
        dec_ref_bits(_py, util_bits);
    }
    if !obj_from_bits(machinery_bits).is_none() {
        dec_ref_bits(_py, machinery_bits);
    }
    out
}

fn importlib_import_via_spec_with_support(
    _py: &PyToken<'_>,
    resolved: &str,
    resolved_bits: u64,
    modules_ptr: *mut u8,
    util_bits: u64,
    machinery_bits: u64,
) -> Result<u64, u64> {
    if let Some(existing_bits) =
        importlib_dict_get_string_key_bits(_py, modules_ptr, resolved_bits)?
    {
        inc_ref_bits(_py, existing_bits);
        return Ok(existing_bits);
    }

    let find_spec_bits = importlib_required_callable(
        _py,
        util_bits,
        runtime_static_name_slot(_py, b"find_spec"),
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

    let preseed_modules =
        importlib_spec_transaction_should_preseed(_py, spec_bits, machinery_bits)?;
    let out_bits = importlib_spec_execution_transaction(
        _py,
        resolved,
        resolved_bits,
        spec_bits,
        modules_ptr,
        ImportlibSpecExecutionOptions {
            reuse_existing: false,
            preseed_new_module: preseed_modules,
            allow_load_module_fallback: true,
        },
    )?;
    if !obj_from_bits(spec_bits).is_none() {
        dec_ref_bits(_py, spec_bits);
    }
    Ok(out_bits)
}

#[derive(Clone, Copy)]
struct ImportlibSpecExecutionOptions {
    reuse_existing: bool,
    preseed_new_module: bool,
    allow_load_module_fallback: bool,
}

fn importlib_spec_transaction_should_preseed(
    _py: &PyToken<'_>,
    spec_bits: u64,
    machinery_bits: u64,
) -> Result<bool, u64> {
    let loader_name = intern_runtime_static_name(_py, b"loader");
    let Some(loader_bits) = getattr_optional_bits(_py, spec_bits, loader_name)? else {
        return Ok(true);
    };
    let out = if obj_from_bits(loader_bits).is_none() {
        true
    } else {
        !importlib_loader_is_molt_loader(_py, loader_bits, machinery_bits)?
    };
    if !obj_from_bits(loader_bits).is_none() {
        dec_ref_bits(_py, loader_bits);
    }
    Ok(out)
}

fn importlib_spec_execution_cleanup(
    _py: &PyToken<'_>,
    modules_ptr: *mut u8,
    name_bits: u64,
    inserted_new_module: bool,
    loader_bits: u64,
    module_bits: u64,
) {
    if inserted_new_module {
        importlib_dict_del_string_key(_py, modules_ptr, name_bits);
    }
    if !obj_from_bits(loader_bits).is_none() {
        dec_ref_bits(_py, loader_bits);
    }
    if !obj_from_bits(module_bits).is_none() {
        dec_ref_bits(_py, module_bits);
    }
}

fn importlib_spec_execution_transaction(
    _py: &PyToken<'_>,
    module_name: &str,
    name_bits: u64,
    spec_bits: u64,
    modules_ptr: *mut u8,
    options: ImportlibSpecExecutionOptions,
) -> Result<u64, u64> {
    let existing_bits = if options.reuse_existing {
        importlib_dict_get_string_key_bits(_py, modules_ptr, name_bits)?
    } else {
        None
    };
    let mut module_bits = if let Some(bits) = existing_bits {
        inc_ref_bits(_py, bits);
        bits
    } else {
        let bits = importlib_ffi::importlib_module_from_spec_impl(_py, spec_bits);
        if exception_pending(_py) {
            return Err(MoltObject::none().bits());
        }
        bits
    };
    let using_existing = existing_bits.is_some();
    let mut inserted_new_module = false;

    if !using_existing && options.preseed_new_module {
        if let Err(err) = importlib_dict_set_string_key(_py, modules_ptr, name_bits, module_bits) {
            importlib_spec_execution_cleanup(
                _py,
                modules_ptr,
                name_bits,
                false,
                MoltObject::none().bits(),
                module_bits,
            );
            return Err(err);
        }
        inserted_new_module = true;
    }

    let loader_name = intern_runtime_static_name(_py, b"loader");
    let loader_attr = match getattr_optional_bits(_py, spec_bits, loader_name) {
        Ok(value) => value,
        Err(err) => {
            importlib_spec_execution_cleanup(
                _py,
                modules_ptr,
                name_bits,
                inserted_new_module,
                MoltObject::none().bits(),
                module_bits,
            );
            return Err(err);
        }
    };
    let loader_bits = loader_attr.unwrap_or_else(|| MoltObject::none().bits());

    if !obj_from_bits(loader_bits).is_none() {
        let exec_name = intern_runtime_static_name(_py, b"exec_module");
        let load_name = intern_runtime_static_name(_py, b"load_module");
        let exec_lookup = match importlib_reader_lookup_callable(_py, loader_bits, exec_name) {
            Ok(value) => value,
            Err(err) => {
                importlib_spec_execution_cleanup(
                    _py,
                    modules_ptr,
                    name_bits,
                    inserted_new_module,
                    loader_bits,
                    module_bits,
                );
                return Err(err);
            }
        };
        if let Some(exec_bits) = exec_lookup {
            let out_bits = unsafe { call_callable1(_py, exec_bits, module_bits) };
            dec_ref_bits(_py, exec_bits);
            if exception_pending(_py) {
                if !obj_from_bits(out_bits).is_none() {
                    dec_ref_bits(_py, out_bits);
                }
                importlib_spec_execution_cleanup(
                    _py,
                    modules_ptr,
                    name_bits,
                    inserted_new_module,
                    loader_bits,
                    module_bits,
                );
                return Err(MoltObject::none().bits());
            }
            if !obj_from_bits(out_bits).is_none() {
                dec_ref_bits(_py, out_bits);
            }
        } else if options.allow_load_module_fallback {
            let load_lookup = match importlib_reader_lookup_callable(_py, loader_bits, load_name) {
                Ok(value) => value,
                Err(err) => {
                    importlib_spec_execution_cleanup(
                        _py,
                        modules_ptr,
                        name_bits,
                        inserted_new_module,
                        loader_bits,
                        module_bits,
                    );
                    return Err(err);
                }
            };
            if let Some(load_bits) = load_lookup {
                let loaded_bits = unsafe { call_callable1(_py, load_bits, name_bits) };
                dec_ref_bits(_py, load_bits);
                if exception_pending(_py) {
                    importlib_spec_execution_cleanup(
                        _py,
                        modules_ptr,
                        name_bits,
                        inserted_new_module,
                        loader_bits,
                        module_bits,
                    );
                    return Err(MoltObject::none().bits());
                }
                if !obj_from_bits(loaded_bits).is_none() {
                    if !obj_from_bits(module_bits).is_none() {
                        dec_ref_bits(_py, module_bits);
                    }
                    module_bits = loaded_bits;
                }
            }
        } else {
            importlib_spec_execution_cleanup(
                _py,
                modules_ptr,
                name_bits,
                inserted_new_module,
                loader_bits,
                module_bits,
            );
            return Err(raise_exception::<_>(_py, "ImportError", ""));
        }
    } else if !options.allow_load_module_fallback {
        importlib_spec_execution_cleanup(
            _py,
            modules_ptr,
            name_bits,
            inserted_new_module,
            loader_bits,
            module_bits,
        );
        return Err(raise_exception::<_>(_py, "ImportError", ""));
    }

    if !using_existing && !options.preseed_new_module {
        if let Err(err) = importlib_dict_set_string_key(_py, modules_ptr, name_bits, module_bits) {
            importlib_spec_execution_cleanup(
                _py,
                modules_ptr,
                name_bits,
                false,
                loader_bits,
                module_bits,
            );
            return Err(err);
        }
        inserted_new_module = true;
    }

    let out_bits = match importlib_dict_get_string_key_bits(_py, modules_ptr, name_bits) {
        Err(err) => {
            importlib_spec_execution_cleanup(
                _py,
                modules_ptr,
                name_bits,
                inserted_new_module,
                loader_bits,
                module_bits,
            );
            return Err(err);
        }
        Ok(None) => {
            inc_ref_bits(_py, module_bits);
            module_bits
        }
        Ok(Some(bits)) => {
            inc_ref_bits(_py, bits);
            bits
        }
    };
    if !obj_from_bits(loader_bits).is_none() {
        dec_ref_bits(_py, loader_bits);
    }
    if !obj_from_bits(module_bits).is_none() {
        dec_ref_bits(_py, module_bits);
    }
    let _ = module_name;
    Ok(out_bits)
}

#[unsafe(no_mangle)]
pub extern "C" fn molt_importlib_load_module_from_spec(
    loader_bits: u64,
    fullname_bits: u64,
    spec_bits: u64,
) -> u64 {
    crate::with_gil_entry_nopanic!(_py, {
        let fullname = match string_arg_from_bits(_py, fullname_bits, "fullname") {
            Ok(value) => value,
            Err(bits) => return bits,
        };
        if obj_from_bits(loader_bits).is_none() {
            return raise_exception::<_>(_py, "ImportError", "");
        }
        let modules_bits = match importlib_runtime_modules_bits(_py) {
            Ok(bits) => bits,
            Err(err) => return err,
        };
        let out = (|| -> Result<u64, u64> {
            let Some(modules_ptr) = obj_from_bits(modules_bits).as_ptr() else {
                return Err(importlib_modules_runtime_error(_py));
            };
            importlib_spec_execution_transaction(
                _py,
                &fullname,
                fullname_bits,
                spec_bits,
                modules_ptr,
                ImportlibSpecExecutionOptions {
                    reuse_existing: true,
                    preseed_new_module: true,
                    allow_load_module_fallback: false,
                },
            )
        })();
        if !obj_from_bits(modules_bits).is_none() {
            dec_ref_bits(_py, modules_bits);
        }
        match out {
            Ok(bits) => bits,
            Err(err) => err,
        }
    })
}

fn importlib_import_with_fallback(
    _py: &PyToken<'_>,
    resolved: &str,
    resolved_bits: u64,
    modules_ptr: *mut u8,
) -> Result<u64, u64> {
    let result = importlib_import_with_fallback_inner(_py, resolved, resolved_bits, modules_ptr);

    // If every import mechanism failed with ModuleNotFoundError, try loading a
    // native C extension (.so / .dylib) from sys.path before giving up.
    #[cfg(all(feature = "cext_loader", not(target_arch = "wasm32")))]
    if result.is_err() && importlib_exception_should_fallback(_py, resolved) {
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
) -> Result<u64, u64> {
    if IMPORTLIB_SPEC_FIRST_IMPORTS.contains(&resolved) {
        return importlib_import_via_spec(_py, resolved, resolved_bits, modules_ptr);
    }

    let module_bits = crate::molt_module_import(resolved_bits);
    if exception_pending(_py) {
        if importlib_exception_should_fallback(_py, resolved) {
            if !obj_from_bits(module_bits).is_none() {
                dec_ref_bits(_py, module_bits);
            }
            return importlib_import_via_spec(_py, resolved, resolved_bits, modules_ptr);
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
            return importlib_import_via_spec(_py, resolved, resolved_bits, modules_ptr);
        }
        return Ok(module_bits);
    }

    clear_exception(_py);
    importlib_import_via_spec(_py, resolved, resolved_bits, modules_ptr)
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
    let sys_bits = importlib_sys_module_bits(_py)?;
    if obj_from_bits(sys_bits).is_none() {
        return None;
    }
    let path_name = intern_runtime_static_name(_py, b"path");
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
    let set_result = match obj_from_bits(parent_bits).as_ptr() {
        Some(parent_ptr) if unsafe { object_type_id(parent_ptr) } == TYPE_ID_MODULE => {
            crate::builtins::modules::molt_module_set_attr(
                parent_bits,
                child_name_bits,
                module_bits,
            )
        }
        _ => molt_object_setattr(parent_bits, child_name_bits, module_bits),
    };
    if !obj_from_bits(set_result).is_none() {
        dec_ref_bits(_py, set_result);
    }
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

#[path = "platform_importlib_support.rs"]
mod importlib_support;
use importlib_support::*;

#[path = "platform_importlib_resources/mod.rs"]
mod importlib_resources;
pub use importlib_resources::*;

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
