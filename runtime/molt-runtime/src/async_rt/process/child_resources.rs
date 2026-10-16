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
    // Child inheritance serializes the typed parent policy. Memory requests
    // use the startup human-size grammar; effective per-operation fallbacks
    // are inherited even when the parent's optional fields were unset.
    "MOLT_RESOURCE_MAX_OPERATION_RESULT",
    "MOLT_RESOURCE_MAX_POW_RESULT",
    "MOLT_RESOURCE_MAX_REPEAT_RESULT",
    "MOLT_RESOURCE_MAX_SHIFT_RESULT",
    "MOLT_RESOURCE_MAX_STRING_RESULT",
];

fn parent_resource_limit(limits: &crate::resource::ResourceLimits, key: &str) -> Option<u128> {
    let operation = limits.operation_result_limits();
    match key {
        "MOLT_RESOURCE_MAX_MEMORY" => limits.max_memory.map(|v| v as u128),
        "MOLT_RESOURCE_MAX_DURATION_MS" => limits.max_duration.map(|v| v.as_millis()),
        "MOLT_RESOURCE_MAX_ALLOCATIONS" => limits.max_allocations.map(|v| v as u128),
        "MOLT_RESOURCE_MAX_RECURSION_DEPTH" => limits.max_recursion_depth.map(|v| v as u128),
        // The four resolved fields below carry the effective fallback. Do not
        // introduce a separate combined default here.
        "MOLT_RESOURCE_MAX_OPERATION_RESULT" => {
            limits.max_operation_result_bytes.map(|v| v as u128)
        }
        "MOLT_RESOURCE_MAX_POW_RESULT" => Some(operation[0] as u128),
        "MOLT_RESOURCE_MAX_REPEAT_RESULT" => Some(operation[1] as u128),
        "MOLT_RESOURCE_MAX_SHIFT_RESULT" => Some(operation[2] as u128),
        "MOLT_RESOURCE_MAX_STRING_RESULT" => Some(operation[3] as u128),
        _ => unreachable!("unknown child resource field"),
    }
}

fn requested_resource_limit(key: &str, raw: &str) -> Option<u128> {
    if key == "MOLT_RESOURCE_MAX_MEMORY" {
        crate::resource::parse_human_size(raw)
            .ok()
            .map(|value| value as u128)
    } else if key == "MOLT_RESOURCE_MAX_DURATION_MS" {
        crate::resource::parse_positive_integer::<u64>(raw)
            .ok()
            .map(|value| value as u128)
    } else {
        crate::resource::parse_positive_integer::<usize>(raw)
            .ok()
            .map(|value| value as u128)
    }
}

// A requested combined cap supplies a missing per-operation request, exactly
// as it does at child startup. An explicit per-operation field wins.
fn combined_request_key(key: &str) -> Option<&'static str> {
    match key {
        "MOLT_RESOURCE_MAX_POW_RESULT"
        | "MOLT_RESOURCE_MAX_REPEAT_RESULT"
        | "MOLT_RESOURCE_MAX_SHIFT_RESULT"
        | "MOLT_RESOURCE_MAX_STRING_RESULT" => Some("MOLT_RESOURCE_MAX_OPERATION_RESULT"),
        _ => None,
    }
}

#[cfg(any(test, target_arch = "wasm32"))]
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
        .find(|(entry_key, _)| {
            #[cfg(windows)]
            {
                entry_key
                    .to_str()
                    .is_some_and(|value| value.eq_ignore_ascii_case(key))
            }
            #[cfg(not(windows))]
            {
                entry_key == key
            }
        })
        .map(|(_, value)| value.as_os_str())
}

fn enforced_child_resource_env_value(
    limits: Option<&crate::resource::ResourceLimits>,
    key: &str,
    requested: Option<&str>,
) -> Option<String> {
    let parent_limit = parent_resource_limit(limits?, key)?;
    let selected = match requested.and_then(|raw| requested_resource_limit(key, raw)) {
        Some(child_limit) if child_limit < parent_limit => child_limit,
        _ => parent_limit,
    };
    Some(selected.to_string())
}

#[cfg(not(target_arch = "wasm32"))]
fn enforced_child_resource_env_os_value(
    limits: Option<&crate::resource::ResourceLimits>,
    key: &str,
    requested: Option<&OsStr>,
) -> Option<String> {
    enforced_child_resource_env_value(limits, key, requested.and_then(OsStr::to_str))
}

#[cfg(not(target_arch = "wasm32"))]
pub(super) fn apply_child_resource_env(
    cmd: &mut Command,
    env_entries: Option<&[(std::ffi::OsString, std::ffi::OsString)]>,
    limits: Option<&crate::resource::ResourceLimits>,
) {
    for key in CHILD_RESOURCE_ENV_KEYS {
        if let Some(value) = enforced_child_resource_env_os_value(
            limits,
            key,
            env_entry_os_value(env_entries, key).or_else(|| {
                combined_request_key(key)
                    .and_then(|fallback| env_entry_os_value(env_entries, fallback))
            }),
        ) {
            cmd.env(key, value);
        }
    }
}

#[cfg(any(test, target_arch = "wasm32"))]
pub(super) fn enforce_child_resource_env_entries(
    entries: &mut Option<Vec<(String, String)>>,
    overlay: &mut bool,
    limits: Option<&crate::resource::ResourceLimits>,
) {
    let mut enforced = Vec::new();
    for key in CHILD_RESOURCE_ENV_KEYS {
        if let Some(value) = enforced_child_resource_env_value(
            limits,
            key,
            env_entry_value(entries.as_deref(), key).or_else(|| {
                combined_request_key(key)
                    .and_then(|fallback| env_entry_value(entries.as_deref(), fallback))
            }),
        ) {
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
fn parse_child_rlimit_gb(raw: &str) -> Option<Option<u64>> {
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

/// OS memory cap for a spawned child. The parent's Layer-1 tracker limit maps
/// to the same backstop budget runtime init installs (limit plus headroom, so a
/// Molt child's own tracker fires first); an explicit `MOLT_CHILD_RLIMIT_GB`
/// operator cap applies exactly and may only tighten it.
#[cfg(unix)]
pub(crate) fn child_memory_rlimit_bytes(
    limits: Option<&crate::resource::ResourceLimits>,
    raw: Option<&str>,
) -> Option<u64> {
    if let Some(None) = raw.and_then(parse_child_rlimit_gb) {
        return None;
    }

    let mut limit = limits
        .and_then(|limits| limits.max_memory)
        .filter(|value| *value > 0)
        .and_then(|value| u64::try_from(crate::resource::memory_backstop_budget(value)).ok());
    if let Some(Some(value)) = raw.and_then(parse_child_rlimit_gb) {
        limit = Some(limit.map_or(value, |current| current.min(value)));
    }
    limit.filter(|value| *value > 0)
}

#[cfg(unix)]
pub(super) fn apply_child_memory_rlimit(cmd: &mut Command, limit: Option<u64>) {
    let Some(limit_bytes) = limit else {
        return;
    };
    apply_child_committed_memory_rlimit(cmd, limit_bytes);
}

/// Linux charges `RLIMIT_DATA` for every writable private mapping, so it caps
/// what the child can commit. `RLIMIT_AS` would also charge sparse
/// reservations (allocator arenas, the mapped executable, guard regions) and
/// break healthy children before they allocate anything.
#[cfg(target_os = "linux")]
fn apply_child_committed_memory_rlimit(cmd: &mut Command, limit_bytes: u64) {
    use std::os::unix::process::CommandExt;
    unsafe {
        cmd.pre_exec(move || {
            let hard_limit: libc::rlim_t =
                molt_codegen_abi::c_width::c_ulong_from_u64_saturating(limit_bytes);
            let limit = libc::rlimit {
                rlim_cur: hard_limit,
                rlim_max: hard_limit,
            };
            if libc::setrlimit(libc::RLIMIT_DATA, &limit) < 0 {
                return Err(std::io::Error::last_os_error());
            }
            Ok(())
        });
    }
}

/// Other Unix kernels have no committed-memory rlimit (macOS's `RLIMIT_DATA`
/// governs only `brk`); the child's own tracker and the parent's guard remain
/// the enforcement.
#[cfg(all(unix, not(target_os = "linux")))]
fn apply_child_committed_memory_rlimit(_cmd: &mut Command, _limit_bytes: u64) {}

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
    use crate::resource::ResourceLimits;

    fn memory(raw: &str) -> ResourceLimits {
        ResourceLimits {
            max_memory: Some(crate::resource::parse_human_size(raw).unwrap()),
            ..Default::default()
        }
    }

    #[test]
    fn child_resource_env_inherits_parent_when_child_omits_limit() {
        let limits = memory("4096");
        assert_eq!(
            enforced_child_resource_env_value(Some(&limits), "MOLT_RESOURCE_MAX_MEMORY", None),
            Some("4096".into())
        );
        assert_eq!(
            enforced_child_resource_env_value(None, "MOLT_RESOURCE_MAX_MEMORY", Some("4096")),
            None
        );
    }

    #[test]
    fn child_resource_env_can_tighten_but_not_widen_parent_limit() {
        let limits = memory("64M");
        for (request, expected) in [
            ("128M", 64 * 1024 * 1024),
            ("32M", 32 * 1024 * 1024),
            ("0", 64 * 1024 * 1024),
            ("invalid", 64 * 1024 * 1024),
        ] {
            assert_eq!(
                enforced_child_resource_env_value(
                    Some(&limits),
                    "MOLT_RESOURCE_MAX_MEMORY",
                    Some(request)
                ),
                Some(expected.to_string())
            );
        }
    }

    #[test]
    fn child_inherits_effective_operation_default_and_tighter_combined_request() {
        let limits = memory("64M");
        let mut command = Command::new("unused-policy-plan");
        let entries = vec![("MOLT_RESOURCE_MAX_OPERATION_RESULT".into(), "1024".into())];
        apply_child_resource_env(&mut command, Some(&entries), Some(&limits));
        let actual: std::collections::HashMap<_, _> = command
            .get_envs()
            .filter_map(|(key, value)| value.map(|value| (key.to_owned(), value.to_owned())))
            .collect();
        for key in [
            "MOLT_RESOURCE_MAX_POW_RESULT",
            "MOLT_RESOURCE_MAX_REPEAT_RESULT",
            "MOLT_RESOURCE_MAX_SHIFT_RESULT",
            "MOLT_RESOURCE_MAX_STRING_RESULT",
        ] {
            assert_eq!(
                actual.get(OsStr::new(key)),
                Some(&std::ffi::OsString::from("1024"))
            );
            assert_eq!(
                enforced_child_resource_env_value(Some(&limits), key, Some("999999999")),
                Some((10 * 1024 * 1024).to_string())
            );
        }
    }

    #[test]
    fn child_policy_does_not_reread_mutated_process_environment() {
        let _transaction = crate::test_support::RuntimeTestTransaction::new();
        let key = "MOLT_RESOURCE_MAX_MEMORY";
        let saved = std::env::var_os(key);
        let limits = memory("64M");
        unsafe {
            std::env::set_var(key, "0");
        }
        let selected = enforced_child_resource_env_value(Some(&limits), key, None);
        match saved {
            Some(value) => unsafe {
                std::env::set_var(key, value);
            },
            None => unsafe {
                std::env::remove_var(key);
            },
        }
        assert_eq!(selected, Some((64 * 1024 * 1024).to_string()));
    }

    #[cfg(windows)]
    #[test]
    fn windows_child_policy_recognizes_ascii_case_variant_requests() {
        let limits = memory("64M");
        let mut command = Command::new("unused-policy-plan");
        let entries = vec![("molt_resource_max_memory".into(), "32M".into())];
        apply_child_resource_env(&mut command, Some(&entries), Some(&limits));
        let selected = command
            .get_envs()
            .find(|(key, _)| {
                key.to_str()
                    .is_some_and(|value| value.eq_ignore_ascii_case("MOLT_RESOURCE_MAX_MEMORY"))
            })
            .unwrap()
            .1;
        assert_eq!(selected, Some(OsStr::new("33554432")));
    }

    #[test]
    fn wasm_child_plan_preserves_tighter_requests_and_effective_defaults() {
        let limits = memory("64M");
        let mut entries = Some(vec![
            ("MOLT_RESOURCE_MAX_MEMORY".into(), "32M".into()),
            ("MOLT_RESOURCE_MAX_POW_RESULT".into(), "999999999".into()),
            ("APPLICATION_VALUE".into(), "visible".into()),
        ]);
        let mut overlay = false;
        enforce_child_resource_env_entries(&mut entries, &mut overlay, Some(&limits));
        assert!(
            !overlay,
            "replacement semantics must survive policy enforcement"
        );
        let actual: std::collections::HashMap<_, _> = entries.unwrap().into_iter().collect();
        assert_eq!(actual["MOLT_RESOURCE_MAX_MEMORY"], "33554432");
        assert_eq!(actual["APPLICATION_VALUE"], "visible");
        for key in [
            "MOLT_RESOURCE_MAX_POW_RESULT",
            "MOLT_RESOURCE_MAX_REPEAT_RESULT",
            "MOLT_RESOURCE_MAX_SHIFT_RESULT",
            "MOLT_RESOURCE_MAX_STRING_RESULT",
        ] {
            assert_eq!(actual[key], "10485760");
        }
        let mut inherited = None;
        enforce_child_resource_env_entries(&mut inherited, &mut overlay, Some(&limits));
        assert!(
            overlay,
            "implicit environment keeps application inheritance"
        );
        assert_eq!(
            env_entry_value(inherited.as_deref(), "MOLT_RESOURCE_MAX_MEMORY"),
            Some("67108864")
        );
        let mut unlimited = None;
        let mut unlimited_overlay = false;
        enforce_child_resource_env_entries(&mut unlimited, &mut unlimited_overlay, None);
        assert!(unlimited.is_none());
        assert!(!unlimited_overlay);
    }

    #[cfg(unix)]
    #[test]
    fn child_memory_rlimit_uses_tightest_runtime_and_shared_limit() {
        assert_eq!(
            child_memory_rlimit_bytes(Some(&memory("8192")), Some("0.000003814697265625")),
            Some(4096)
        );
    }

    #[cfg(unix)]
    #[test]
    fn child_memory_rlimit_keeps_backstop_headroom_over_runtime_limit() {
        assert_eq!(
            child_memory_rlimit_bytes(Some(&memory("64M")), None),
            Some(crate::resource::memory_backstop_budget(64 * 1024 * 1024) as u64)
        );
    }

    #[cfg(unix)]
    #[test]
    fn child_memory_rlimit_zero_shared_limit_disables_os_limit() {
        assert_eq!(
            child_memory_rlimit_bytes(Some(&memory("4096")), Some("0")),
            None
        );
    }
}
