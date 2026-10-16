#[cfg(target_arch = "wasm32")]
use crate::libc_compat as libc;
use std::collections::HashSet;
use std::ffi::{CStr, CString, OsStr, OsString};
use std::sync::Mutex;

// libc locale strings and environment pointers are borrowed process state.
// Copy results under this lock; never retain them or invoke Python while held.
static PROCESS_LOCALE_LOCK: Mutex<()> = Mutex::new(());

#[derive(Debug, PartialEq, Eq)]
pub enum EnvironmentError {
    Value(&'static str),
    Errno(i32),
}

/// Serialize every Molt process-environment mutation with snapshots and locale.
/// Python mapping ownership is separate; never hold this lock across callbacks.
pub fn process_env_set(key: &OsStr, value: Option<&OsStr>) -> Result<(), EnvironmentError> {
    let key_bytes = key.as_encoded_bytes();
    // CPython allows Windows hidden names such as =C:, but no later '='.
    let forbidden_equals = if cfg!(windows) && key_bytes.first() == Some(&b'=') {
        key_bytes[1..].contains(&b'=')
    } else {
        key_bytes.contains(&b'=')
    };
    #[cfg(windows)]
    if forbidden_equals || key.is_empty() {
        return Err(EnvironmentError::Value("illegal environment variable name"));
    }
    #[cfg(not(windows))]
    if key.as_encoded_bytes().contains(&0)
        || value.is_some_and(|value| value.as_encoded_bytes().contains(&0))
    {
        return Err(EnvironmentError::Value(if cfg!(windows) {
            "embedded null character"
        } else {
            "embedded null byte"
        }));
    }
    #[cfg(not(windows))]
    if forbidden_equals {
        // putenv validates '=' explicitly; unsetenv delegates it to libc,
        // which reports EINVAL. Avoid Rust remove_var's panic for that input.
        return Err(if value.is_some() {
            EnvironmentError::Value("illegal environment variable name")
        } else {
            EnvironmentError::Errno(libc::EINVAL)
        });
    }
    if key.is_empty() {
        // Rust set_var panics on an empty POSIX key; CPython reports setenv's
        // EINVAL as OSError, distinct from its explicit name validation.
        return Err(EnvironmentError::Errno(libc::EINVAL));
    }
    let _lock = PROCESS_LOCALE_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    process_env_set_unlocked(key, value)
}

fn process_env_set_unlocked(key: &OsStr, value: Option<&OsStr>) -> Result<(), EnvironmentError> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let mut assignment: Vec<u16> = key.encode_wide().collect();
        assignment.push(b'=' as u16);
        if let Some(value) = value {
            assignment.extend(value.encode_wide());
        }
        if assignment.len() > 32767 {
            return Err(EnvironmentError::Value(
                "the environment variable is longer than 32767 characters",
            ));
        }
        // CPython validates name, then UTF-16 assignment length, then NUL.
        if assignment.contains(&0) {
            return Err(EnvironmentError::Value("embedded null character"));
        }
        assignment.push(0);
        // Match CPython's CRT/PEB writer, including hidden leading-'=' keys.
        let rc = unsafe { crate::windows_abi::_wputenv(assignment.as_ptr()) };
        if rc == 0 {
            Ok(())
        } else {
            Err(EnvironmentError::Errno(unsafe {
                *crate::windows_abi::_errno()
            }))
        }
    }
    #[cfg(all(not(windows), not(target_arch = "wasm32")))]
    {
        // Participate in Rust's process-environment lock used by var_os,
        // vars_os and Command. Direct libc writes would race those readers.
        // This remains a release hold: std aborts on a libc-reported write
        // failure and its panic can disclose the assignment. Rust has no public
        // fallible writer under ENV_LOCK. Foreign C access also requires the
        // same embedding coordination as CPython; this lock cannot protect it.
        unsafe {
            if let Some(value) = value {
                std::env::set_var(key, value);
            } else {
                std::env::remove_var(key);
            }
        }
        Ok(())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let key = CString::new(key.as_encoded_bytes())
            .map_err(|_| EnvironmentError::Value("embedded null byte"))?;
        let rc = if let Some(value) = value {
            let value = CString::new(value.as_encoded_bytes())
                .map_err(|_| EnvironmentError::Value("embedded null byte"))?;
            unsafe { libc::setenv(key.as_ptr(), value.as_ptr(), 1) }
        } else {
            unsafe { libc::unsetenv(key.as_ptr()) }
        };
        if rc == 0 {
            Ok(())
        } else {
            Err(EnvironmentError::Errno(
                std::io::Error::last_os_error()
                    .raw_os_error()
                    .expect("libc setenv/unsetenv reports errno"),
            ))
        }
    }
}

fn process_env_get_unlocked(key: &str) -> Option<OsString> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        std::env::var_os(key)
    }
    #[cfg(target_arch = "wasm32")]
    {
        let key = CString::new(key).ok()?;
        let ptr = unsafe { libc::getenv(key.as_ptr()) };
        if ptr.is_null() {
            return None;
        }
        Some(unsafe {
            OsString::from_encoded_bytes_unchecked(CStr::from_ptr(ptr).to_bytes().to_vec())
        })
    }
}

fn set_locale_unlocked(category: i32, locale: Option<&str>) -> Result<String, String> {
    #[cfg(windows)]
    {
        if !(libc::LC_ALL..=libc::LC_TIME).contains(&category) {
            return Err("invalid locale category".into());
        }
        let value = locale.map(|value| value.encode_utf16().chain(Some(0)).collect::<Vec<_>>());
        if value
            .as_ref()
            .is_some_and(|value| value[..value.len() - 1].contains(&0))
        {
            return Err("embedded null character".into());
        }
        let ptr = unsafe {
            crate::windows_abi::_wsetlocale(
                category,
                value
                    .as_ref()
                    .map_or(std::ptr::null(), |value| value.as_ptr()),
            )
        };
        if ptr.is_null() {
            return Err("unsupported locale setting".into());
        }
        let mut len = 0;
        unsafe {
            while *ptr.add(len) != 0 {
                len += 1;
            }
        }
        String::from_utf16(unsafe { std::slice::from_raw_parts(ptr, len) })
            .map_err(|_| "invalid locale name".into())
    }
    #[cfg(not(windows))]
    {
        let value = locale
            .map(CString::new)
            .transpose()
            .map_err(|_| "embedded null character")?;
        let ptr = unsafe {
            libc::setlocale(
                category,
                value
                    .as_ref()
                    .map_or(std::ptr::null(), |value| value.as_ptr()),
            )
        };
        if ptr.is_null() {
            return Err("unsupported locale setting".into());
        }
        unsafe { CStr::from_ptr(ptr) }
            .to_str()
            .map(str::to_owned)
            .map_err(|_| "invalid locale name".into())
    }
}

fn locale_codeset_unlocked() -> Result<String, String> {
    #[cfg(windows)]
    {
        Ok(format!("cp{}", unsafe { crate::windows_abi::GetACP() }))
    }
    #[cfg(not(windows))]
    {
        let ptr = unsafe { libc::nl_langinfo(libc::CODESET) };
        if ptr.is_null() {
            return Err("locale encoding is unavailable".into());
        }
        let value = unsafe { CStr::from_ptr(ptr) }
            .to_str()
            .map_err(|_| "invalid locale encoding")?;
        Ok(value.to_owned())
    }
}

fn locale_encoding_unlocked() -> Result<String, String> {
    let codeset = locale_codeset_unlocked()?;
    // _Py_GetLocaleEncoding permits empty CODESET after locale selection;
    // coercion probes must inspect the actual CODESET before accepting it.
    Ok(if codeset.is_empty() {
        "utf-8".to_owned()
    } else {
        codeset
    })
}

pub fn set_locale(category: i32, locale: Option<&str>) -> Result<String, String> {
    let _lock = PROCESS_LOCALE_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    set_locale_unlocked(category, locale)
}

pub fn locale_encoding() -> Result<String, String> {
    let _lock = PROCESS_LOCALE_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    locale_encoding_unlocked()
}

/// Windows user-default locale metadata is independent of mutable CRT locale.
/// ISO names and the legacy hexadecimal LCID fields are ASCII by Win32 contract.
#[cfg(windows)]
pub fn windows_default_locale(legacy_lcid: bool) -> (Option<String>, String) {
    let code_page = unsafe { crate::windows_abi::GetACP() };
    windows_default_locale_with(code_page, legacy_lcid, |kind, output| unsafe {
        crate::windows_abi::GetLocaleInfoA(
            crate::windows_abi::LOCALE_USER_DEFAULT,
            kind,
            output.as_mut_ptr(),
            output.len() as i32,
        )
    })
}

#[cfg(windows)]
fn windows_default_locale_with(
    code_page: u32,
    legacy_lcid: bool,
    mut query: impl FnMut(u32, &mut [u8]) -> i32,
) -> (Option<String>, String) {
    use crate::windows_abi::{
        LOCALE_IDEFAULTLANGUAGE, LOCALE_SISO639LANGNAME, LOCALE_SISO3166CTRYNAME,
    };
    let encoding = format!("cp{code_page}");
    let mut locale = [0u8; 100];
    let language_len = query(LOCALE_SISO639LANGNAME, &mut locale);
    if language_len > 0 {
        // GetLocaleInfoA's successful length includes the trailing NUL.
        let separator = language_len as usize - 1;
        locale[separator] = b'_';
        let country_len = query(LOCALE_SISO3166CTRYNAME, &mut locale[separator + 1..]);
        if country_len > 0 {
            let end = separator + country_len as usize;
            let name = std::str::from_utf8(&locale[..end])
                .expect("Win32 ISO locale names are ASCII")
                .to_owned();
            return (Some(name), encoding);
        }
    }
    // CPython 3.15 retired the Windows-95 language-ID fallback. Its absence
    // is a provider result, not an OS exception or a POSIX environment query.
    if legacy_lcid {
        locale[..2].copy_from_slice(b"0x");
        let count = query(LOCALE_IDEFAULTLANGUAGE, &mut locale[2..]);
        if count > 0 {
            let name = std::str::from_utf8(&locale[..count as usize + 1])
                .expect("Win32 locale language IDs are hexadecimal ASCII")
                .to_owned();
            return (Some(name), encoding);
        }
    }
    (None, encoding)
}

/// Immutable per-runtime selection. Later setlocale calls affect current
/// locale encoding, but cannot rewrite flags, filesystem, or standard streams.
#[derive(Clone, Debug)]
pub struct StartupEncoding {
    pub utf8_mode: bool,
    pub warn_default_encoding: bool,
    pub filesystem_encoding: String,
    pub filesystem_errors: &'static str,
    pub stdio_encoding: String,
    pub stdio_errors: String,
}

pub fn startup_encoding() -> Result<StartupEncoding, String> {
    let _lock = PROCESS_LOCALE_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    // A failed environment locale selection leaves libc's current locale in
    // place, matching CPython's startup selection before preconfiguration.
    let _ = set_locale_unlocked(libc::LC_CTYPE, Some(""));
    let initial_locale = set_locale_unlocked(libc::LC_CTYPE, None)?;
    let nonempty_env = |name| process_env_get_unlocked(name).filter(|value| !value.is_empty());
    let legacy_windows_fs = cfg!(windows)
        && nonempty_env("PYTHONLEGACYWINDOWSFSENCODING").is_some_and(|value| {
            // CPython's environment flag accepts a decimal C int; malformed
            // and negative values enable the flag, while numeric zero does not.
            value.to_str().and_then(|value| {
                value
                    .trim_start_matches(|ch: char| ch.is_ascii_whitespace())
                    .parse::<i32>()
                    .ok()
            }) != Some(0)
        });
    let utf8_mode = if legacy_windows_fs {
        false
    } else {
        match nonempty_env("PYTHONUTF8").as_deref() {
            Some(value) if value == OsStr::new("0") => false,
            Some(value) if value == OsStr::new("1") => true,
            Some(_) => return Err("invalid PYTHONUTF8 environment variable value".into()),
            None => !cfg!(windows) && matches!(initial_locale.as_str(), "C" | "POSIX"),
        }
    };
    let coerce = nonempty_env("PYTHONCOERCECLOCALE");
    if !cfg!(windows)
        && initial_locale == "C"
        && coerce.as_deref() != Some(OsStr::new("0"))
        && nonempty_env("LC_ALL").is_none()
    {
        for candidate in ["C.UTF-8", "C.utf8", "UTF-8"] {
            if set_locale_unlocked(libc::LC_CTYPE, Some(candidate)).is_ok()
                && locale_codeset_unlocked().is_ok_and(|codeset| !codeset.is_empty())
            {
                // CPython resets the candidate before publishing LC_CTYPE.
                // An OS publication failure skips coercion and continues startup.
                let _ = set_locale_unlocked(libc::LC_ALL, Some(""));
                if process_env_set_unlocked(OsStr::new("LC_CTYPE"), Some(OsStr::new(candidate)))
                    .is_err()
                {
                    eprintln!("Error setting LC_CTYPE, skipping C locale coercion");
                    break;
                }
                let _ = set_locale_unlocked(libc::LC_ALL, Some(""));
                if coerce.as_deref() == Some(OsStr::new("warn")) {
                    eprintln!(
                        "Python detected LC_CTYPE=C: LC_CTYPE coerced to {candidate} (set another locale or PYTHONCOERCECLOCALE=0 to disable this locale coercion behavior)."
                    );
                }
                break;
            }
            // A libc can admit a locale without providing a usable CODESET.
            // Do not leave that failed coercion candidate selected.
            set_locale_unlocked(libc::LC_CTYPE, Some(&initial_locale))?;
        }
    }
    let current_locale = set_locale_unlocked(libc::LC_CTYPE, None)?;
    if coerce.as_deref() == Some(OsStr::new("warn")) && current_locale == "C" {
        eprintln!(
            "Python runtime initialized with LC_CTYPE=C (a locale with default ASCII encoding), which may cause Unicode compatibility problems. Using C.UTF-8, C.utf8, or UTF-8 (if available) as alternative Unicode-compatible locales is recommended."
        );
    }
    let current_encoding = locale_encoding_unlocked()?;
    let default_encoding = if utf8_mode {
        "utf-8".to_owned()
    } else {
        current_encoding.clone()
    };
    let filesystem_encoding = if legacy_windows_fs {
        "mbcs".to_owned()
    } else if cfg!(windows) || cfg!(target_vendor = "apple") || utf8_mode {
        "utf-8".to_owned()
    } else {
        current_encoding
    };
    let filesystem_errors = if legacy_windows_fs {
        "replace"
    } else if cfg!(windows) {
        "surrogatepass"
    } else {
        "surrogateescape"
    };
    let default_errors = if cfg!(windows)
        || utf8_mode
        || matches!(
            current_locale.as_str(),
            "C" | "POSIX" | "C.UTF-8" | "C.utf8" | "UTF-8"
        ) {
        "surrogateescape"
    } else {
        "strict"
    };
    let mut stdio_encoding = default_encoding;
    let mut stdio_errors = default_errors.to_owned();
    if let Some(value) = nonempty_env("PYTHONIOENCODING") {
        let value = value
            .into_string()
            .map_err(|_| "invalid PYTHONIOENCODING environment variable value")?;
        let (encoding, errors) = value.split_once(':').unwrap_or((&value, ""));
        if !encoding.is_empty() {
            stdio_encoding = encoding.to_owned();
            stdio_errors = "strict".to_owned();
        }
        if !errors.is_empty() {
            stdio_errors = errors.to_owned();
        }
    }
    Ok(StartupEncoding {
        utf8_mode,
        warn_default_encoding: nonempty_env("PYTHONWARNDEFAULTENCODING").is_some(),
        filesystem_encoding,
        filesystem_errors,
        stdio_encoding,
        stdio_errors,
    })
}

/// Copy the raw CPython provider in order, retaining the first exact key.
/// The temporary snapshot is not the Python mapping and is never retained here.
/// All first-party runtime writers must use process_env_set; foreign writers
/// require embedding coordination. No Python allocation/callback runs locked.
pub fn process_env_snapshot() -> Vec<(OsString, OsString)> {
    let _lock = PROCESS_LOCALE_LOCK
        .lock()
        .unwrap_or_else(|p| p.into_inner());
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStringExt;
        unsafe {
            // CPython uses the CRT array, not GetEnvironmentStringsW. Initialize
            // it for main() hosts before obtaining the current array pointer.
            crate::windows_abi::_wgetenv([0u16].as_ptr());
            let mut entries = *crate::windows_abi::__p__wenviron();
            collect_provider_entries(
                std::iter::from_fn(|| {
                    if entries.is_null() || (*entries).is_null() {
                        return None;
                    }
                    let pointer = *entries;
                    entries = entries.add(1);
                    let mut len = 0;
                    while *pointer.add(len) != 0 {
                        len += 1;
                    }
                    Some(std::slice::from_raw_parts(pointer, len))
                }),
                b'=' as u16,
                OsString::from_wide,
            )
        }
    }
    #[cfg(not(windows))]
    {
        #[cfg(target_arch = "wasm32")]
        let mut entries = unsafe { libc::__wasilibc_get_environ() };
        #[cfg(all(not(target_arch = "wasm32"), target_vendor = "apple"))]
        let mut entries = unsafe { *libc::_NSGetEnviron() };
        #[cfg(all(not(target_arch = "wasm32"), not(target_vendor = "apple")))]
        let mut entries = unsafe {
            unsafe extern "C" {
                static mut environ: *mut *mut libc::c_char;
            }
            environ
        };
        unsafe {
            collect_provider_entries(
                std::iter::from_fn(|| {
                    if entries.is_null() || (*entries).is_null() {
                        return None;
                    }
                    let entry = CStr::from_ptr(*entries).to_bytes();
                    entries = entries.add(1);
                    Some(entry)
                }),
                b'=',
                |bytes| OsString::from_encoded_bytes_unchecked(bytes.to_vec()),
            )
        }
    }
}

fn collect_provider_entries<'a, Unit: Eq + std::hash::Hash + 'a>(
    entries: impl Iterator<Item = &'a [Unit]>,
    separator: Unit,
    to_os_string: impl Fn(&[Unit]) -> OsString,
) -> Vec<(OsString, OsString)> {
    // Borrow keys only while the provider is locked. Duplicate rows require no
    // key/value copy; the returned vector owns every accepted byte/code unit.
    let mut seen = HashSet::new();
    let mut out = Vec::new();
    for entry in entries {
        if let Some(at) = entry.iter().position(|unit| *unit == separator)
            && seen.insert(&entry[..at])
        {
            out.push((to_os_string(&entry[..at]), to_os_string(&entry[at + 1..])));
        }
    }
    out
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

    #[cfg(not(windows))]
    #[test]
    fn provider_snapshot_preserves_first_separator_first_duplicate_and_order() {
        let rows: &[&[u8]] = &[
            b"=X",
            b"=Y=Z",
            b"=",
            b"A=1",
            b"A=2",
            b"malformed",
            b"B=a=b",
            b"C=",
            b"\xff=\xfe",
        ];
        let result = collect_provider_entries(rows.iter().copied(), b'=', |bytes| unsafe {
            OsString::from_encoded_bytes_unchecked(bytes.to_vec())
        });
        let bytes: Vec<_> = result
            .iter()
            .map(|(key, value)| (key.as_encoded_bytes(), value.as_encoded_bytes()))
            .collect();
        assert_eq!(
            bytes,
            vec![
                (b"".as_slice(), b"X".as_slice()),
                (b"A".as_slice(), b"1".as_slice()),
                (b"B".as_slice(), b"a=b".as_slice()),
                (b"C".as_slice(), b"".as_slice()),
                (b"\xff".as_slice(), b"\xfe".as_slice())
            ]
        );
    }

    #[test]
    fn provider_snapshot_wide_parser_keeps_exact_case_and_empty_names() {
        let rows: Vec<Vec<u16>> = [
            "=C:=C:\\",
            "Path=first",
            "PATH=second",
            "Path=ignored",
            "malformed",
        ]
        .iter()
        .map(|s| s.encode_utf16().collect())
        .collect();
        let result =
            collect_provider_entries(rows.iter().map(Vec::as_slice), b'=' as u16, |units| {
                OsString::from(String::from_utf16(units).unwrap())
            });
        assert_eq!(
            result,
            vec![
                (OsString::from(""), OsString::from("C:=C:\\")),
                (OsString::from("Path"), OsString::from("first")),
                (OsString::from("PATH"), OsString::from("second"))
            ]
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn process_writer_rejects_invalid_inputs_without_mutating_or_panicking() {
        assert_eq!(
            process_env_set(OsStr::new("A=B"), Some(OsStr::new("value"))),
            Err(EnvironmentError::Value("illegal environment variable name"))
        );
        assert_eq!(
            process_env_set(OsStr::new("A=B"), None),
            Err(EnvironmentError::Errno(libc::EINVAL))
        );
        assert_eq!(
            process_env_set(OsStr::new(""), Some(OsStr::new("value"))),
            Err(EnvironmentError::Errno(libc::EINVAL))
        );
        assert_eq!(
            process_env_set(OsStr::new(""), None),
            Err(EnvironmentError::Errno(libc::EINVAL))
        );
        assert_eq!(
            process_env_set(OsStr::new("A\0B"), None),
            Err(EnvironmentError::Value("embedded null byte"))
        );
        assert_eq!(
            process_env_set(OsStr::new("A"), Some(OsStr::new("B\0C"))),
            Err(EnvironmentError::Value("embedded null byte"))
        );
    }
}

#[cfg(all(test, windows))]
mod windows_validation_order_tests {
    use super::*;
    use std::os::windows::ffi::OsStringExt;

    #[test]
    fn default_locale_preserves_iso_lcid_and_unavailable_provider_order() {
        use crate::windows_abi::{
            LOCALE_IDEFAULTLANGUAGE, LOCALE_SISO639LANGNAME, LOCALE_SISO3166CTRYNAME,
        };
        let scenarios: &[(&[Option<&[u8]>], Option<&str>, &[u32], &[usize])] = &[
            (
                &[Some(b"en\0"), Some(b"US\0")],
                Some("en_US"),
                &[LOCALE_SISO639LANGNAME, LOCALE_SISO3166CTRYNAME],
                &[100, 97],
            ),
            (
                &[None, Some(b"0409\0")],
                Some("0x0409"),
                &[LOCALE_SISO639LANGNAME, LOCALE_IDEFAULTLANGUAGE],
                &[100, 98],
            ),
            (
                &[Some(b"en\0"), None, Some(b"0409\0")],
                Some("0x0409"),
                &[
                    LOCALE_SISO639LANGNAME,
                    LOCALE_SISO3166CTRYNAME,
                    LOCALE_IDEFAULTLANGUAGE,
                ],
                &[100, 97, 98],
            ),
            (
                &[None, None],
                None,
                &[LOCALE_SISO639LANGNAME, LOCALE_IDEFAULTLANGUAGE],
                &[100, 98],
            ),
        ];
        for &(responses, expected, kinds, sizes) in scenarios {
            let mut calls = Vec::new();
            let result = windows_default_locale_with(932, true, |kind, output| {
                let response = responses[calls.len()];
                calls.push((kind, output.len()));
                if let Some(bytes) = response {
                    output[..bytes.len()].copy_from_slice(bytes);
                    bytes.len() as i32
                } else {
                    0
                }
            });
            assert_eq!(result, (expected.map(str::to_owned), "cp932".into()));
            assert_eq!(
                calls,
                kinds
                    .iter()
                    .copied()
                    .zip(sizes.iter().copied())
                    .collect::<Vec<_>>()
            );
        }
        // 3.15 does not ask for the legacy LCID after either ISO query fails.
        for country_failure in [false, true] {
            let mut calls = Vec::new();
            let result = windows_default_locale_with(65001, false, |kind, output| {
                calls.push(kind);
                assert_ne!(kind, LOCALE_IDEFAULTLANGUAGE);
                if country_failure && kind == LOCALE_SISO639LANGNAME {
                    output[..3].copy_from_slice(b"en\0");
                    3
                } else {
                    0
                }
            });
            assert_eq!(result, (None, "cp65001".into()));
            assert_eq!(
                calls,
                if country_failure {
                    vec![LOCALE_SISO639LANGNAME, LOCALE_SISO3166CTRYNAME]
                } else {
                    vec![LOCALE_SISO639LANGNAME]
                }
            );
        }
    }

    #[test]
    fn cpython_name_then_assignment_length_then_nul_precedence() {
        let long_nul = OsString::from_wide(&vec![0; 32767]);
        assert_eq!(
            process_env_set(OsStr::new("A=B"), Some(&long_nul)),
            Err(EnvironmentError::Value("illegal environment variable name"))
        );
        assert_eq!(
            process_env_set(OsStr::new("A"), Some(&long_nul)),
            Err(EnvironmentError::Value(
                "the environment variable is longer than 32767 characters"
            ))
        );
        let short_nul = OsString::from_wide(&[0]);
        assert_eq!(
            process_env_set(OsStr::new("A"), Some(&short_nul)),
            Err(EnvironmentError::Value("embedded null character"))
        );
    }
}
