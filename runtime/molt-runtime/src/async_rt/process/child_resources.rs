#[cfg(unix)]
use crate::libc_compat as libc;
#[cfg(not(target_arch = "wasm32"))]
use std::ffi::OsStr;
#[cfg(not(target_arch = "wasm32"))]
use std::process::Command;

const CHILD_RESOURCE_ENV_KEYS: &[&str] = &[
    "MOLT_RESOURCE_MAX_MEMORY",
    "MOLT_RESOURCE_MAX_DURATION_MS",
    "MOLT_RESOURCE_MAX_ALLOCATIONS",
    "MOLT_RESOURCE_MAX_RECURSION_DEPTH",
    // Per-operation result caps are raw-byte integers like the keys above, so a
    // spawned child inherits the tighter of (parent, child-requested) for each.
    // The `MOLT_MEMORY_LIMIT` human-size alias is intentionally absent: a child
    // resolves it into `max_memory` at its own init, and the numeric min-merge
    // here only handles raw integers.
    "MOLT_RESOURCE_MAX_OPERATION_RESULT",
    "MOLT_RESOURCE_MAX_POW_RESULT",
    "MOLT_RESOURCE_MAX_REPEAT_RESULT",
    "MOLT_RESOURCE_MAX_SHIFT_RESULT",
    "MOLT_RESOURCE_MAX_STRING_RESULT",
];

fn parse_resource_limit(raw: &str) -> Option<u128> {
    raw.trim().parse::<u128>().ok()
}

fn active_parent_resource_limit(key: &str) -> Option<u128> {
    std::env::var(key)
        .ok()
        .and_then(|raw| parse_resource_limit(&raw))
}

#[cfg(target_arch = "wasm32")]
fn env_entry_value<'a>(entries: Option<&'a [(String, String)]>, key: &str) -> Option<&'a str> {
    entries?
        .iter()
        .rev()
        .find(|(entry_key, _)| entry_key == key)
        .map(|(_, value)| value.as_str())
}

#[cfg(not(target_arch = "wasm32"))]
fn env_entry_os_value<'a>(
    entries: Option<&'a [(std::ffi::OsString, std::ffi::OsString)]>,
    key: &str,
) -> Option<&'a OsStr> {
    entries?
        .iter()
        .rev()
        .find(|(entry_key, _)| entry_key == key)
        .map(|(_, value)| value.as_os_str())
}

fn enforced_child_resource_env_value(key: &str, requested: Option<&str>) -> Option<String> {
    let parent_limit = active_parent_resource_limit(key)?;
    let selected = match requested.and_then(parse_resource_limit) {
        Some(child_limit) if child_limit < parent_limit => child_limit,
        _ => parent_limit,
    };
    Some(selected.to_string())
}

#[cfg(not(target_arch = "wasm32"))]
fn enforced_child_resource_env_os_value(key: &str, requested: Option<&OsStr>) -> Option<String> {
    enforced_child_resource_env_value(key, requested.and_then(OsStr::to_str))
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn apply_child_resource_env(
    cmd: &mut Command,
    env_entries: Option<&[(std::ffi::OsString, std::ffi::OsString)]>,
) {
    for key in CHILD_RESOURCE_ENV_KEYS {
        if let Some(value) =
            enforced_child_resource_env_os_value(key, env_entry_os_value(env_entries, key))
        {
            cmd.env(key, value);
        }
    }
}

#[cfg(target_arch = "wasm32")]
pub(super) fn enforce_child_resource_env_entries(
    entries: &mut Option<Vec<(String, String)>>,
    overlay: &mut bool,
) {
    let mut enforced = Vec::new();
    for key in CHILD_RESOURCE_ENV_KEYS {
        if let Some(value) =
            enforced_child_resource_env_value(key, env_entry_value(entries.as_deref(), key))
        {
            enforced.push(((*key).to_string(), value));
        }
    }
    if enforced.is_empty() {
        return;
    }
    let entries = entries.get_or_insert_with(|| {
        *overlay = true;
        Vec::new()
    });
    for (key, value) in enforced {
        entries.retain(|(entry_key, _)| entry_key != &key);
        entries.push((key, value));
    }
}

#[cfg(unix)]
fn parse_child_rlimit_bytes_env(name: &str) -> Option<Option<u64>> {
    let raw = std::env::var(name).ok()?;
    let value = raw.trim().parse::<u64>().ok()?;
    if value == 0 {
        Some(None)
    } else {
        Some(Some(value))
    }
}

#[cfg(unix)]
fn parse_child_rlimit_gb_env(name: &str) -> Option<Option<u64>> {
    let raw = std::env::var(name).ok()?;
    let value = raw.trim().parse::<f64>().ok()?;
    if value == 0.0 {
        return Some(None);
    }
    if !value.is_finite() || value < 0.0 {
        return None;
    }
    let bytes = value * 1024.0 * 1024.0 * 1024.0;
    if bytes > u64::MAX as f64 {
        return None;
    }
    Some(Some(bytes as u64))
}

#[cfg(unix)]
fn child_memory_rlimit_bytes() -> Option<u64> {
    for candidate in [
        parse_child_rlimit_bytes_env("MOLT_CHILD_RLIMIT_BYTES"),
        parse_child_rlimit_gb_env("MOLT_CHILD_RLIMIT_GB"),
    ] {
        if let Some(None) = candidate {
            return None;
        }
    }

    let mut limit = active_parent_resource_limit("MOLT_RESOURCE_MAX_MEMORY")
        .and_then(|value| u64::try_from(value).ok());
    for candidate in [
        parse_child_rlimit_bytes_env("MOLT_CHILD_RLIMIT_BYTES"),
        parse_child_rlimit_gb_env("MOLT_CHILD_RLIMIT_GB"),
    ] {
        if let Some(Some(value)) = candidate {
            limit = Some(limit.map_or(value, |current| current.min(value)));
        }
    }
    limit.filter(|value| *value > 0)
}

#[cfg(unix)]
pub(super) fn apply_child_memory_rlimit(cmd: &mut Command) {
    let Some(limit_bytes) = child_memory_rlimit_bytes() else {
        return;
    };
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(move || {
            let hard_limit = limit_bytes.min(libc::rlim_t::MAX as u64) as libc::rlim_t;
            let limit = libc::rlimit {
                rlim_cur: hard_limit,
                rlim_max: hard_limit,
            };
            if libc::setrlimit(libc::RLIMIT_AS, &limit) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

#[cfg(unix)]
pub(super) fn configure_unix_owned_process_group(
    cmd: &mut Command,
    start_new_session: bool,
    process_group: Option<i64>,
) -> bool {
    use std::os::unix::process::CommandExt;

    let setpgid_target = match (start_new_session, process_group) {
        (true, None | Some(0)) => None,
        (true, Some(pgid)) => Some(pgid),
        (false, Some(pgid)) => Some(pgid),
        (false, None) => Some(0),
    };
    let owns_group = start_new_session || setpgid_target == Some(0);
    if start_new_session || setpgid_target.is_some() {
        unsafe {
            cmd.pre_exec(move || {
                if start_new_session && libc::setsid() < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                if let Some(pgid) = setpgid_target
                    && libc::setpgid(0, pgid as libc::pid_t) < 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
    }
    owns_group
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    fn with_env<R>(updates: &[(&str, Option<&str>)], f: impl FnOnce() -> R) -> R {
        // Use the single process-wide test mutex shared with the resource-env
        // tests (resource.rs, ops_sys.rs). These suites mutate the SAME
        // MOLT_RESOURCE_MAX_* env vars; a private mutex here would let them race
        // and clobber each other's env across module boundaries.
        let _guard = crate::test_mutex_guard();
        let saved = updates
            .iter()
            .map(|(key, _)| (*key, std::env::var(key).ok()))
            .collect::<Vec<_>>();
        for (key, value) in updates {
            match value {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
        let result = f();
        for (key, value) in saved {
            match value {
                Some(value) => unsafe { std::env::set_var(key, value) },
                None => unsafe { std::env::remove_var(key) },
            }
        }
        result
    }

    #[test]
    fn child_resource_env_inherits_parent_when_child_omits_limit() {
        with_env(
            &[
                ("MOLT_RESOURCE_MAX_MEMORY", Some("4096")),
                ("MOLT_CHILD_RLIMIT_BYTES", None),
                ("MOLT_CHILD_RLIMIT_GB", None),
            ],
            || {
                assert_eq!(
                    enforced_child_resource_env_value("MOLT_RESOURCE_MAX_MEMORY", None),
                    Some("4096".to_string())
                );
            },
        );
    }

    #[test]
    fn child_resource_env_can_tighten_but_not_widen_parent_limit() {
        with_env(&[("MOLT_RESOURCE_MAX_MEMORY", Some("4096"))], || {
            assert_eq!(
                enforced_child_resource_env_value("MOLT_RESOURCE_MAX_MEMORY", Some("8192")),
                Some("4096".to_string())
            );
            assert_eq!(
                enforced_child_resource_env_value("MOLT_RESOURCE_MAX_MEMORY", Some("1024")),
                Some("1024".to_string())
            );
            assert_eq!(
                enforced_child_resource_env_value("MOLT_RESOURCE_MAX_MEMORY", Some("invalid")),
                Some("4096".to_string())
            );
        });
    }

    #[cfg(unix)]
    #[test]
    fn child_memory_rlimit_uses_tightest_runtime_and_shared_limit() {
        with_env(
            &[
                ("MOLT_RESOURCE_MAX_MEMORY", Some("8192")),
                ("MOLT_CHILD_RLIMIT_BYTES", Some("4096")),
                ("MOLT_CHILD_RLIMIT_GB", None),
            ],
            || {
                assert_eq!(child_memory_rlimit_bytes(), Some(4096));
            },
        );
    }

    #[cfg(unix)]
    #[test]
    fn child_memory_rlimit_zero_shared_limit_disables_os_limit() {
        with_env(
            &[
                ("MOLT_RESOURCE_MAX_MEMORY", Some("4096")),
                ("MOLT_CHILD_RLIMIT_BYTES", None),
                ("MOLT_CHILD_RLIMIT_GB", Some("0")),
            ],
            || {
                assert_eq!(child_memory_rlimit_bytes(), None);
            },
        );
    }
}
