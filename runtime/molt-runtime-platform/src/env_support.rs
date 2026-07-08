use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};

static ENV_STATE: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
static PROCESS_ENV_STATE: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
static LOCALE_STATE: OnceLock<Mutex<String>> = OnceLock::new();

pub fn trace_env_get() -> bool {
    static TRACE: OnceLock<bool> = OnceLock::new();
    *TRACE.get_or_init(|| {
        matches!(
            std::env::var("MOLT_TRACE_ENV_GET").ok().as_deref(),
            Some("1")
        )
    })
}

pub fn env_state() -> &'static Mutex<BTreeMap<String, String>> {
    ENV_STATE.get_or_init(|| Mutex::new(collect_env_state()))
}

pub fn env_state_get(key: &str) -> Option<String> {
    let guard = env_state()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    guard.get(key).cloned()
}

pub fn process_env_state() -> &'static Mutex<BTreeMap<String, String>> {
    PROCESS_ENV_STATE.get_or_init(|| Mutex::new(collect_env_state()))
}

pub fn locale_state() -> &'static Mutex<String> {
    LOCALE_STATE.get_or_init(|| Mutex::new(String::from("C")))
}

pub fn collect_env_state() -> BTreeMap<String, String> {
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

pub fn os_name_str() -> &'static str {
    #[cfg(target_os = "windows")]
    {
        "nt"
    }
    #[cfg(not(target_os = "windows"))]
    {
        "posix"
    }
}

pub fn sys_platform_str() -> &'static str {
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

pub fn locale_encoding_label(locale: &str) -> &'static str {
    if locale == "C" || locale == "POSIX" {
        "US-ASCII"
    } else {
        "UTF-8"
    }
}

#[cfg(all(target_arch = "wasm32", not(feature = "wasm_freestanding")))]
#[link(wasm_import_module = "wasi_snapshot_preview1")]
unsafe extern "C" {
    fn environ_sizes_get(environ_count: *mut u32, environ_buf_size: *mut u32) -> u16;
    fn environ_get(environ: *mut *mut u8, environ_buf: *mut u8) -> u16;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn platform_labels_match_target_contract() {
        assert_eq!(
            os_name_str(),
            if cfg!(target_os = "windows") {
                "nt"
            } else {
                "posix"
            }
        );
        if cfg!(target_arch = "wasm32") {
            assert_eq!(sys_platform_str(), "wasi");
        } else if cfg!(target_os = "windows") {
            assert_eq!(sys_platform_str(), "win32");
        } else if cfg!(target_os = "macos") {
            assert_eq!(sys_platform_str(), "darwin");
        } else if cfg!(target_os = "linux") {
            assert_eq!(sys_platform_str(), "linux");
        }
    }

    #[test]
    fn locale_encoding_labels_match_runtime_contract() {
        assert_eq!(locale_encoding_label("C"), "US-ASCII");
        assert_eq!(locale_encoding_label("POSIX"), "US-ASCII");
        assert_eq!(locale_encoding_label("en_US.UTF-8"), "UTF-8");
    }

    #[test]
    fn env_state_get_reads_shared_snapshot() {
        let key = "__MOLT_ENV_SUPPORT_TEST_KEY__";
        let original = {
            let mut guard = env_state()
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let original = guard.get(key).cloned();
            guard.insert(key.to_string(), "value".to_string());
            original
        };
        assert_eq!(env_state_get(key), Some("value".to_string()));
        let mut guard = env_state()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match original {
            Some(value) => {
                guard.insert(key.to_string(), value);
            }
            None => {
                guard.remove(key);
            }
        }
    }
}
