//! The backend process's memory budget: one authority for every consumer that
//! sizes work or caches by available memory (TIR optimization batching, the
//! pass cache, the backend process guard).
//!
//! The memory guard writes these values as fractional GB with six decimals,
//! so every reader parses them as non-negative `f64` GB.

const GIB_BYTES: u64 = 1024 * 1024 * 1024;

/// Memory available to the backend, most specific first. The guard's
/// process cap is the last fallback.
const AVAILABLE_GB_ENV: [&str; 4] = [
    "MOLT_BACKEND_MEMORY_AVAILABLE_GB",
    "MOLT_CLI_MEMORY_AVAILABLE_GB",
    "MOLT_MEMORY_AVAILABLE_GB",
    "MOLT_BACKEND_MAX_PROCESS_RSS_GB",
];

/// Memory to leave untouched, most specific first.
const RESERVE_GB_ENV: [&str; 3] = [
    "MOLT_BACKEND_MEMORY_RESERVE_GB",
    "MOLT_CLI_MEMORY_RESERVE_GB",
    "MOLT_MEMORY_RESERVE_GB",
];

/// Parse a non-negative, possibly fractional GB value into bytes, saturating
/// at `u64::MAX`.
pub fn parse_nonnegative_gb(raw: &str) -> Option<u64> {
    let gb = raw
        .trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0)?;
    Some((gb * GIB_BYTES as f64).min(u64::MAX as f64) as u64)
}

fn first_gb_env(names: &[&str]) -> Option<u64> {
    names.iter().find_map(|name| {
        std::env::var(name)
            .ok()
            .and_then(|raw| parse_nonnegative_gb(&raw))
    })
}

/// Available memory minus the reserve, from the environment.
pub fn env_memory_budget_bytes() -> Option<u64> {
    let available = first_gb_env(&AVAILABLE_GB_ENV)?;
    let reserve = first_gb_env(&RESERVE_GB_ENV).unwrap_or(0);
    Some(available.saturating_sub(reserve))
}

/// The committed-memory rlimit the backend process guard installs
/// (`RLIMIT_DATA` on Linux; other kernels have none).
#[cfg(target_os = "linux")]
pub fn rlimit_committed_memory_bytes() -> Option<u64> {
    let mut limit = libc::rlimit {
        rlim_cur: 0,
        rlim_max: 0,
    };
    // SAFETY: `getrlimit` writes only into the `rlimit` it is handed.
    if unsafe { libc::getrlimit(libc::RLIMIT_DATA, &mut limit) } != 0 {
        return None;
    }
    let raw = limit.rlim_cur;
    if raw == libc::RLIM_INFINITY || raw == 0 {
        return None;
    }
    Some(u128::from(raw).min(u128::from(u64::MAX)) as u64)
}

#[cfg(not(target_os = "linux"))]
pub fn rlimit_committed_memory_bytes() -> Option<u64> {
    None
}

/// The tighter of the environment budget and the committed-memory rlimit.
pub fn backend_memory_limit_bytes() -> Option<u64> {
    match (env_memory_budget_bytes(), rlimit_committed_memory_bytes()) {
        (Some(env_limit), Some(rlimit)) => Some(env_limit.min(rlimit)),
        (env_limit, rlimit) => env_limit.or(rlimit),
    }
}

#[cfg(test)]
mod tests {
    use super::{GIB_BYTES, parse_nonnegative_gb};

    #[test]
    fn fractional_guard_values_parse_to_bytes() {
        assert_eq!(parse_nonnegative_gb("1.500000"), Some(3 * GIB_BYTES / 2));
        assert_eq!(parse_nonnegative_gb(" 12 "), Some(12 * GIB_BYTES));
        assert_eq!(parse_nonnegative_gb("0"), Some(0));
        assert_eq!(parse_nonnegative_gb("1e30"), Some(u64::MAX));
    }

    #[test]
    fn negative_non_finite_and_malformed_values_are_absent() {
        for raw in ["-1", "inf", "NaN", "", "2G", "not-a-number"] {
            assert_eq!(parse_nonnegative_gb(raw), None, "{raw:?}");
        }
    }
}
