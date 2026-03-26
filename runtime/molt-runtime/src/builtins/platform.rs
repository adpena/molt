use std::collections::BTreeMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use digest::Digest;
use md5::Md5;
use sha1::Sha1;

#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
use crate::randomness::fill_os_random;
use crate::*;

// Re-export importlib items so tests and other modules see them via `use super::*`.
#[allow(unused_imports)]
use super::platform_importlib::*;

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
pub(super) static EXTENSION_METADATA_OK_CACHE: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
pub(super) static EXTENSION_METADATA_CACHE_HITS: AtomicU64 = AtomicU64::new(0);
pub(super) static EXTENSION_METADATA_CACHE_MISSES: AtomicU64 = AtomicU64::new(0);
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

pub(super) fn env_state() -> &'static Mutex<BTreeMap<String, String>> {
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

pub(super) fn extension_metadata_ok_cache() -> &'static Mutex<BTreeMap<String, String>> {
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

pub(super) fn sys_platform_str() -> &'static str {
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

    fn extension_boundary_module_filename(module_basename: &str) -> String {
        if sys_platform_str().starts_with("win") {
            format!("{module_basename}.pyd")
        } else {
            format!("{module_basename}.so")
        }
    }

    fn clear_extension_metadata_validation_cache() {
        let cache = extension_metadata_ok_cache();
        let mut guard = cache
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        guard.clear();
        EXTENSION_METADATA_CACHE_HITS.store(0, Ordering::Relaxed);
        EXTENSION_METADATA_CACHE_MISSES.store(0, Ordering::Relaxed);
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

    fn extension_spec_bits_for_tests(_py: &PyToken<'_>, module_name: &str, origin: &str) -> u64 {
        let spec_bits = unsafe { call_callable0(_py, builtin_classes(_py).object) };
        assert!(
            !obj_from_bits(spec_bits).is_none(),
            "failed to create synthetic spec object"
        );
        assert!(
            !exception_pending(_py),
            "failed to instantiate synthetic spec object: {:?}",
            pending_exception_kind_and_message(_py)
        );
        let module_name_bits = alloc_test_string_bits(_py, module_name);
        let rc = unsafe {
            crate::c_api::molt_object_setattr_bytes(
                spec_bits,
                b"name".as_ptr(),
                b"name".len() as u64,
                module_name_bits,
            )
        };
        assert_eq!(
            rc,
            0,
            "set synthetic spec name failed: {:?}",
            pending_exception_kind_and_message(_py)
        );
        dec_ref_bits(_py, module_name_bits);
        let origin_bits = alloc_test_string_bits(_py, origin);
        let rc = unsafe {
            crate::c_api::molt_object_setattr_bytes(
                spec_bits,
                b"origin".as_ptr(),
                b"origin".len() as u64,
                origin_bits,
            )
        };
        assert_eq!(
            rc,
            0,
            "set synthetic spec origin failed: {:?}",
            pending_exception_kind_and_message(_py)
        );
        dec_ref_bits(_py, origin_bits);
        spec_bits
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
    fn extension_spec_boundary_rejects_missing_manifest_sidecar() {
        with_trusted_runtime(|| {
            clear_extension_metadata_validation_cache();
            let tmp = extension_boundary_temp_dir("molt_extension_spec_missing_manifest");
            std::fs::create_dir_all(&tmp).expect("create temp dir");
            let module_name = format!("ext_spec_missing_{}", std::process::id());
            let filename = extension_boundary_module_filename(&module_name);
            let extension_path = tmp.join(&filename);
            std::fs::write(&extension_path, b"spec-boundary-extension")
                .expect("write extension placeholder");
            let search_paths = vec![tmp.to_string_lossy().into_owned()];

            crate::with_gil_entry!(_py, {
                let out = importlib_find_spec_payload(
                    _py,
                    &module_name,
                    &search_paths,
                    None,
                    1,
                    0,
                    false,
                );
                assert!(
                    out.is_err(),
                    "expected spec boundary failure for missing manifest"
                );
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
    fn extension_spec_boundary_rejects_invalid_manifest_payload() {
        with_trusted_runtime(|| {
            clear_extension_metadata_validation_cache();
            let tmp = extension_boundary_temp_dir("molt_extension_spec_invalid_manifest");
            std::fs::create_dir_all(&tmp).expect("create temp dir");
            let module_name = format!("ext_spec_invalid_{}", std::process::id());
            let filename = extension_boundary_module_filename(&module_name);
            let extension_path = tmp.join(&filename);
            std::fs::write(&extension_path, b"spec-boundary-extension")
                .expect("write extension placeholder");
            std::fs::write(tmp.join("extension_manifest.json"), b"{not-json}\n")
                .expect("write invalid metadata manifest");
            let search_paths = vec![tmp.to_string_lossy().into_owned()];

            crate::with_gil_entry!(_py, {
                let out = importlib_find_spec_payload(
                    _py,
                    &module_name,
                    &search_paths,
                    None,
                    1,
                    0,
                    false,
                );
                assert!(
                    out.is_err(),
                    "expected spec boundary failure for invalid manifest"
                );
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
    fn extension_spec_boundary_accepts_valid_manifest() {
        with_trusted_runtime(|| {
            clear_extension_metadata_validation_cache();
            let tmp = extension_boundary_temp_dir("molt_extension_spec_valid_manifest");
            std::fs::create_dir_all(&tmp).expect("create temp dir");
            let module_name = format!("ext_spec_valid_{}", std::process::id());
            let filename = extension_boundary_module_filename(&module_name);
            let extension_path = tmp.join(&filename);
            std::fs::write(&extension_path, b"spec-boundary-extension")
                .expect("write extension placeholder");
            let extension_path_text = extension_path.to_string_lossy().into_owned();
            let extension_sha256 =
                importlib_sha256_file(&extension_path_text).expect("hash extension placeholder");
            write_valid_extension_manifest(
                &tmp.join("extension_manifest.json"),
                &module_name,
                &filename,
                &extension_sha256,
            );
            let search_paths = vec![tmp.to_string_lossy().into_owned()];

            crate::with_gil_entry!(_py, {
                let payload = importlib_find_spec_payload(
                    _py,
                    &module_name,
                    &search_paths,
                    None,
                    1,
                    0,
                    false,
                )
                .expect("spec boundary should pass")
                .expect("extension spec should resolve");
                assert_eq!(payload.loader_kind, "extension");
                assert_eq!(
                    payload.origin.as_deref(),
                    Some(extension_path_text.as_str())
                );
                assert!(
                    !exception_pending(_py),
                    "unexpected pending exception after valid spec boundary check: {:?}",
                    pending_exception_kind_and_message(_py)
                );
            });

            std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
        });
    }

    #[test]
    fn extension_spec_boundary_rejects_manifest_module_mismatch() {
        with_trusted_runtime(|| {
            clear_extension_metadata_validation_cache();
            let tmp = extension_boundary_temp_dir("molt_extension_spec_module_mismatch");
            std::fs::create_dir_all(&tmp).expect("create temp dir");
            let module_name = format!("ext_spec_requested_{}", std::process::id());
            let manifest_module_name = format!("ext_spec_manifest_{}", std::process::id());
            let filename = extension_boundary_module_filename(&module_name);
            let extension_path = tmp.join(&filename);
            std::fs::write(&extension_path, b"spec-boundary-extension")
                .expect("write extension placeholder");
            let extension_path_text = extension_path.to_string_lossy().into_owned();
            let extension_sha256 =
                importlib_sha256_file(&extension_path_text).expect("hash extension placeholder");
            write_valid_extension_manifest(
                &tmp.join("extension_manifest.json"),
                &manifest_module_name,
                &filename,
                &extension_sha256,
            );
            let search_paths = vec![tmp.to_string_lossy().into_owned()];

            crate::with_gil_entry!(_py, {
                let out = importlib_find_spec_payload(
                    _py,
                    &module_name,
                    &search_paths,
                    None,
                    1,
                    0,
                    false,
                );
                assert!(
                    out.is_err(),
                    "expected spec boundary failure for manifest module mismatch"
                );
                assert_pending_exception_contains(
                    _py,
                    "ImportError",
                    &["extension metadata module mismatch"],
                );
            });

            std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
        });
    }

    #[test]
    fn extension_spec_boundary_revalidates_cache_after_artifact_mutation() {
        with_trusted_runtime(|| {
            clear_extension_metadata_validation_cache();
            let tmp = extension_boundary_temp_dir("molt_extension_spec_cache_revalidation");
            std::fs::create_dir_all(&tmp).expect("create temp dir");
            let module_name = format!("ext_spec_cache_{}", std::process::id());
            let filename = extension_boundary_module_filename(&module_name);
            let extension_path = tmp.join(&filename);
            std::fs::write(&extension_path, b"spec-boundary-extension-v1")
                .expect("write extension placeholder");
            let extension_path_text = extension_path.to_string_lossy().into_owned();
            let extension_sha256 =
                importlib_sha256_file(&extension_path_text).expect("hash extension placeholder");
            write_valid_extension_manifest(
                &tmp.join("extension_manifest.json"),
                &module_name,
                &filename,
                &extension_sha256,
            );
            let search_paths = vec![tmp.to_string_lossy().into_owned()];

            crate::with_gil_entry!(_py, {
                let payload = importlib_find_spec_payload(
                    _py,
                    &module_name,
                    &search_paths,
                    None,
                    1,
                    0,
                    false,
                )
                .expect("first spec boundary pass should succeed")
                .expect("first extension spec should resolve");
                assert_eq!(payload.loader_kind, "extension");
                assert_eq!(
                    payload.origin.as_deref(),
                    Some(extension_path_text.as_str())
                );
            });

            std::fs::write(&extension_path, b"spec-boundary-extension-v2-changed")
                .expect("mutate extension artifact");

            crate::with_gil_entry!(_py, {
                let out = importlib_find_spec_payload(
                    _py,
                    &module_name,
                    &search_paths,
                    None,
                    1,
                    0,
                    false,
                );
                assert!(
                    out.is_err(),
                    "expected spec boundary failure after extension mutation"
                );
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
    fn extension_spec_object_boundary_enforces_missing_and_valid_manifest() {
        with_trusted_runtime(|| {
            clear_extension_metadata_validation_cache();
            {
                let tmp =
                    extension_boundary_temp_dir("molt_extension_spec_object_missing_manifest");
                std::fs::create_dir_all(&tmp).expect("create temp dir");
                let module_name = format!("ext_spec_object_missing_{}", std::process::id());
                let filename = extension_boundary_module_filename(&module_name);
                let extension_path = tmp.join(&filename);
                std::fs::write(&extension_path, b"spec-object-boundary-extension")
                    .expect("write extension placeholder");
                let extension_path_text = extension_path.to_string_lossy().into_owned();

                crate::with_gil_entry!(_py, {
                    let spec_bits =
                        extension_spec_bits_for_tests(_py, &module_name, &extension_path_text);
                    let out = importlib_enforce_extension_spec_object_boundary(
                        _py,
                        &module_name,
                        spec_bits,
                    );
                    assert!(
                        out.is_err(),
                        "expected extension spec object boundary failure for missing manifest"
                    );
                    assert_pending_exception_contains(
                        _py,
                        "ImportError",
                        &[
                            "extension metadata missing",
                            "extension_manifest.json not found near extension path",
                        ],
                    );
                    dec_ref_bits(_py, spec_bits);
                });

                std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
            }

            clear_extension_metadata_validation_cache();

            {
                let tmp = extension_boundary_temp_dir("molt_extension_spec_object_valid_manifest");
                std::fs::create_dir_all(&tmp).expect("create temp dir");
                let module_name = format!("ext_spec_object_valid_{}", std::process::id());
                let filename = extension_boundary_module_filename(&module_name);
                let extension_path = tmp.join(&filename);
                std::fs::write(&extension_path, b"spec-object-boundary-extension")
                    .expect("write extension placeholder");
                let extension_path_text = extension_path.to_string_lossy().into_owned();
                let extension_sha256 = importlib_sha256_file(&extension_path_text)
                    .expect("hash extension placeholder");
                write_valid_extension_manifest(
                    &tmp.join("extension_manifest.json"),
                    &module_name,
                    &filename,
                    &extension_sha256,
                );

                crate::with_gil_entry!(_py, {
                    let spec_bits =
                        extension_spec_bits_for_tests(_py, &module_name, &extension_path_text);
                    let out = importlib_enforce_extension_spec_object_boundary(
                        _py,
                        &module_name,
                        spec_bits,
                    );
                    dec_ref_bits(_py, spec_bits);
                    assert!(
                        out.is_ok(),
                        "unexpected extension spec object boundary failure: {:?}",
                        pending_exception_kind_and_message(_py)
                    );
                    assert!(
                        !exception_pending(_py),
                        "unexpected pending exception after successful spec object boundary check: {:?}",
                        pending_exception_kind_and_message(_py)
                    );
                });

                std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
            }
        });
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
    fn extension_loader_boundary_rejects_manifest_module_mismatch() {
        with_trusted_runtime(|| {
            clear_extension_metadata_validation_cache();
            let tmp = extension_boundary_temp_dir("molt_extension_loader_module_mismatch");
            std::fs::create_dir_all(&tmp).expect("create temp dir");
            let extension_path = tmp.join(extension_boundary_filename());
            std::fs::write(&extension_path, b"loader-boundary-extension")
                .expect("write extension placeholder");
            let module_name = "demo.extension.loader.requested";
            let manifest_module_name = "demo.extension.loader.manifest";
            let extension_path_text = extension_path.to_string_lossy().into_owned();
            let extension_sha256 =
                importlib_sha256_file(&extension_path_text).expect("hash extension placeholder");
            write_valid_extension_manifest(
                &tmp.join("extension_manifest.json"),
                manifest_module_name,
                extension_boundary_filename(),
                &extension_sha256,
            );

            crate::with_gil_entry!(_py, {
                let _ = call_extension_loader_boundary(_py, module_name, &extension_path_text);
                assert_pending_exception_contains(
                    _py,
                    "ImportError",
                    &["extension metadata module mismatch"],
                );
            });

            std::fs::remove_dir_all(&tmp).expect("cleanup temp dir");
        });
    }

    #[test]
    fn extension_exec_boundary_rejects_manifest_module_mismatch() {
        with_trusted_runtime(|| {
            clear_extension_metadata_validation_cache();
            let tmp = extension_boundary_temp_dir("molt_extension_exec_module_mismatch");
            std::fs::create_dir_all(&tmp).expect("create temp dir");
            let extension_path = tmp.join(extension_boundary_filename());
            std::fs::write(&extension_path, b"exec-boundary-extension")
                .expect("write extension placeholder");
            let module_name = "demo.extension.exec.requested";
            let manifest_module_name = "demo.extension.exec.manifest";
            let extension_path_text = extension_path.to_string_lossy().into_owned();
            let extension_sha256 =
                importlib_sha256_file(&extension_path_text).expect("hash extension placeholder");
            write_valid_extension_manifest(
                &tmp.join("extension_manifest.json"),
                manifest_module_name,
                extension_boundary_filename(),
                &extension_sha256,
            );

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
                    &["extension metadata module mismatch"],
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
    fn extension_loader_boundary_records_cache_hits_and_misses() {
        with_trusted_runtime(|| {
            clear_extension_metadata_validation_cache();
            let tmp = extension_boundary_temp_dir("molt_extension_loader_cache_hit_miss");
            std::fs::create_dir_all(&tmp).expect("create temp dir");
            let extension_path = tmp.join(extension_boundary_filename());
            let module_name = "demo.extension.loader.cache_stats";
            std::fs::write(&extension_path, b"loader-boundary-extension-cache")
                .expect("write extension placeholder");
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
                    "unexpected boundary exception on cold validation: {:?}",
                    pending_exception_kind_and_message(_py)
                );
                assert!(
                    !obj_from_bits(payload_bits).is_none(),
                    "expected loader payload on cold validation"
                );
                dec_ref_bits(_py, payload_bits);

                let payload_bits =
                    call_extension_loader_boundary(_py, module_name, &extension_path_text);
                assert!(
                    !exception_pending(_py),
                    "unexpected boundary exception on warm validation: {:?}",
                    pending_exception_kind_and_message(_py)
                );
                assert!(
                    !obj_from_bits(payload_bits).is_none(),
                    "expected loader payload on warm validation"
                );
                dec_ref_bits(_py, payload_bits);
            });

            let (hits, misses) = extension_metadata_cache_stats();
            assert!(
                misses >= 1,
                "expected at least one cold miss, observed hits={hits}, misses={misses}"
            );
            assert!(
                hits >= 1,
                "expected at least one warm hit, observed hits={hits}, misses={misses}"
            );

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

#[cfg(all(target_arch = "wasm32", not(feature = "wasm_freestanding")))]
#[link(wasm_import_module = "wasi_snapshot_preview1")]
unsafe extern "C" {
    fn environ_sizes_get(environ_count: *mut u32, environ_buf_size: *mut u32) -> u16;
    fn environ_get(environ: *mut *mut u8, environ_buf: *mut u8) -> u16;
}
