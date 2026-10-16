//! Integration tests for resource enforcement through the alloc path.
//!
//! These tests verify that the ResourceTracker is actually called during
//! heap allocation and that memory limits are enforced.

molt_runtime::declare_app_bootstrap!(molt_runtime::AppBootstrapProvider::Unavailable(
    "molt-runtime/resource_enforcement"
));

use molt_runtime::resource::{
    LimitedTracker, ResourceLimits, ResourceTracker, UnlimitedTracker, install_memory_backstop,
    parse_human_size, set_tracker, with_tracker,
};

#[allow(dead_code)]
mod cargo_test_artifacts {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_support/cargo_test_artifacts.rs"
    ));
}
mod captured_runtime_children {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_support/captured_runtime_children.rs"
    ));
}

unsafe extern "C" {
    fn molt_runtime_init() -> u64;
    fn molt_runtime_shutdown() -> u64;
}

/// Runtime init installs the process-wide OS memory backstop. Tests that drive
/// the real init path restore the runner's own limit so sibling tests keep it.
struct OsBackstopRestore {
    #[cfg(target_os = "linux")]
    data: libc::rlimit,
}

impl OsBackstopRestore {
    fn capture() -> Self {
        #[cfg(target_os = "linux")]
        {
            let mut data = libc::rlimit {
                rlim_cur: 0,
                rlim_max: 0,
            };
            assert_eq!(unsafe { libc::getrlimit(libc::RLIMIT_DATA, &mut data) }, 0);
            Self { data }
        }
        #[cfg(not(target_os = "linux"))]
        Self {}
    }
}

impl Drop for OsBackstopRestore {
    fn drop(&mut self) {
        #[cfg(target_os = "linux")]
        unsafe {
            libc::setrlimit(libc::RLIMIT_DATA, &self.data);
        }
    }
}

const RESOURCE_ENV: &[&str] = &[
    "MOLT_RESOURCE_MAX_MEMORY",
    "MOLT_RESOURCE_MAX_DURATION_MS",
    "MOLT_RESOURCE_MAX_ALLOCATIONS",
    "MOLT_RESOURCE_MAX_RECURSION_DEPTH",
    "MOLT_RESOURCE_MAX_OPERATION_RESULT",
    "MOLT_RESOURCE_MAX_POW_RESULT",
    "MOLT_RESOURCE_MAX_REPEAT_RESULT",
    "MOLT_RESOURCE_MAX_SHIFT_RESULT",
    "MOLT_RESOURCE_MAX_STRING_RESULT",
];

fn run_startup_child(child: &str, values: &[(&str, &str)]) {
    let mut command = std::process::Command::new(std::env::current_exe().unwrap());
    command.args([
        "--exact",
        child,
        "--ignored",
        "--nocapture",
        "--test-threads=1",
    ]);
    for key in RESOURCE_ENV {
        command.env_remove(key);
    }
    command.envs(values.iter().copied());
    let output = captured_runtime_children::capture(&mut command, "resource-startup", child);
    assert!(
        output.status.success(),
        "{child}: {}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

/// Exercise the installed tracker without allocating a large host buffer. The
/// oracle allows real bootstrap charges and balances every successful charge.
fn assert_startup_budget(memory_limited: bool, allocation_limited: bool) {
    use molt_runtime::resource::ResourceError;
    const MEMORY: usize = 64 * 1024 * 1024;
    with_tracker(|tracker| {
        if memory_limited {
            assert!(matches!(
                tracker.on_grow(MEMORY + 1),
                Err(ResourceError::Memory { limit: MEMORY, .. })
            ));
            assert!(tracker.on_grow(1024 * 1024).is_ok());
            tracker.on_shrink(1024 * 1024);
        } else {
            assert!(tracker.on_grow(256 * 1024 * 1024).is_ok());
            tracker.on_shrink(256 * 1024 * 1024);
        }
        if allocation_limited {
            let mut accepted = 0;
            let mut rejection = None;
            for _ in 0..100_001 {
                match tracker.on_allocate(0) {
                    Ok(()) => accepted += 1,
                    Err(error) => {
                        rejection = Some(error);
                        break;
                    }
                }
            }
            for _ in 0..accepted {
                tracker.on_free(0);
            }
            assert!(
                accepted > 0,
                "bootstrap must leave room under the declared budget"
            );
            assert!(matches!(
                rejection,
                Some(ResourceError::Allocation {
                    limit: 100_000,
                    count: 100_001
                })
            ));
        }
    });
}

fn startup_contract(memory_limited: bool, allocation_limited: bool) {
    let _restore = OsBackstopRestore::capture();
    assert_eq!(unsafe { molt_runtime_init() }, 1);
    assert_startup_budget(memory_limited, allocation_limited);
    // Both widening a cap and adding a cap to an unlimited runtime must leave
    // the already published policy unchanged on the public repeated-init path.
    unsafe {
        std::env::set_var(
            "MOLT_RESOURCE_MAX_MEMORY",
            if memory_limited { "1G" } else { "64M" },
        );
        std::env::set_var("MOLT_RESOURCE_MAX_ALLOCATIONS", "1000000");
    }
    assert_eq!(unsafe { molt_runtime_init() }, 1);
    assert_startup_budget(memory_limited, allocation_limited);
    assert_eq!(unsafe { molt_runtime_shutdown() }, 1);
}

#[test]
fn tracker_receives_allocations() {
    // Install a tracker with a very high limit (won't trigger)
    let limits = ResourceLimits {
        max_memory: Some(1_000_000_000), // 1GB — won't be hit
        ..Default::default()
    };
    set_tracker(Box::new(LimitedTracker::new(&limits)));

    // Do some work that allocates
    let mut v: Vec<u64> = Vec::with_capacity(100);
    for i in 0..100 {
        v.push(i);
    }

    // The tracker should have recorded some memory usage
    // (We can't inspect it directly, but we can verify it doesn't crash)
    drop(v);

    // Reset to unlimited
    set_tracker(Box::new(UnlimitedTracker));
}

#[test]
fn limited_tracker_basics() {
    let limits = ResourceLimits {
        max_memory: Some(1024),
        max_allocations: Some(5),
        ..Default::default()
    };
    let mut tracker = LimitedTracker::new(&limits);

    // First few allocations should succeed
    assert!(tracker.on_allocate(100).is_ok());
    assert!(tracker.on_allocate(100).is_ok());
    assert!(tracker.on_allocate(100).is_ok());

    // Should still have room
    assert!(tracker.on_allocate(100).is_ok());
    assert!(tracker.on_allocate(100).is_ok());

    // 6th allocation should fail (max_allocations=5)
    assert!(tracker.on_allocate(100).is_err());
}

#[test]
fn limited_tracker_memory_limit() {
    let limits = ResourceLimits {
        max_memory: Some(500),
        ..Default::default()
    };
    let mut tracker = LimitedTracker::new(&limits);

    assert!(tracker.on_allocate(200).is_ok());
    assert!(tracker.on_allocate(200).is_ok());
    // 400 bytes used, 100 remaining
    assert!(tracker.on_allocate(200).is_err()); // would be 600 > 500
}

#[test]
fn env_var_init_installs_tracker() {
    run_startup_child(
        "env_var_init_installs_tracker_child",
        &[
            ("MOLT_RESOURCE_MAX_MEMORY", "67108864"),
            ("MOLT_RESOURCE_MAX_ALLOCATIONS", "100000"),
        ],
    );
}

#[test]
#[ignore = "real runtime lifecycle; invoked by its owning subprocess test"]
fn env_var_init_installs_tracker_child() {
    startup_contract(true, true);
}

#[test]
fn molt_memory_limit_alias_enforces_via_real_init_path() {
    run_startup_child(
        "molt_memory_limit_alias_enforces_via_real_init_path_child",
        &[("MOLT_RESOURCE_MAX_MEMORY", "64M")],
    );
}

#[test]
#[ignore = "real runtime lifecycle; invoked by its owning subprocess test"]
fn molt_memory_limit_alias_enforces_via_real_init_path_child() {
    startup_contract(true, false);
}

#[test]
fn no_memory_limit_env_means_unchanged_behavior() {
    run_startup_child("no_memory_limit_env_means_unchanged_behavior_child", &[]);
}

#[test]
#[ignore = "real runtime lifecycle; invoked by its owning subprocess test"]
fn no_memory_limit_env_means_unchanged_behavior_child() {
    startup_contract(false, false);
}

/// Paired with the 64M child above: both spellings reject at exactly 64 MiB.
#[test]
fn human_size_and_raw_bytes_resolve_to_one_limit() {
    run_startup_child(
        "human_size_and_raw_bytes_resolve_to_one_limit_child",
        &[("MOLT_RESOURCE_MAX_MEMORY", "67108864")],
    );
}

#[test]
#[ignore = "real runtime lifecycle; invoked by its owning subprocess test"]
fn human_size_and_raw_bytes_resolve_to_one_limit_child() {
    startup_contract(true, false);
}

#[test]
fn human_size_parser_accepts_documented_forms() {
    assert_eq!(parse_human_size("536870912").unwrap(), 512 * 1024 * 1024);
    assert_eq!(parse_human_size("64M").unwrap(), 64 * 1024 * 1024);
    assert_eq!(parse_human_size("2G").unwrap(), 2 * 1024 * 1024 * 1024);
    assert!(parse_human_size("bogus").is_err());
}

/// The OS memory backstop installs above the live footprint on Linux. A 1 TiB
/// tracker limit keeps the runner's own budget effectively unbounded.
#[cfg(target_os = "linux")]
#[test]
fn memory_backstop_installs_on_linux() {
    let _restore = OsBackstopRestore::capture();
    let installed = install_memory_backstop(1usize << 40);
    assert!(
        installed.is_some_and(
            |bytes| bytes > molt_runtime::resource::memory_backstop_budget(1usize << 40)
        ),
        "RLIMIT_DATA backstop should install above the live footprint on Linux"
    );
}

/// Off Linux no committed-memory rlimit exists; the backstop honestly reports
/// that it is unavailable and the in-VM tracker remains the enforcement.
#[cfg(not(target_os = "linux"))]
#[test]
fn memory_backstop_is_unavailable_off_linux() {
    assert!(install_memory_backstop(1usize << 40).is_none());
}

/// LINUX ONLY: the backstop GENUINELY bounds committed memory without breaking
/// a healthy process. In a forked child (so the runner's own limits are never
/// touched) a 16 MiB tracker limit is installed; the child then proves that
///
/// * `getrlimit(RLIMIT_DATA)` reflects a tightened, finite soft limit;
/// * ordinary growth within the budget still maps and touches memory — the
///   regression an address-space cap caused: this binary's allocator arenas
///   already exceed a small `RLIMIT_AS`, so every new mapping failed and the
///   next main-stack growth was SIGSEGV;
/// * a writable reservation past the budget fails at the OS layer — a clean
///   failure, not an OOM-kill of the host.
#[cfg(target_os = "linux")]
#[test]
fn memory_backstop_bounds_growth_without_breaking_child() {
    // SAFETY: between fork() and _exit() the child only calls libc and our own
    // pure-Rust helpers; it holds no locks taken before the fork.
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0, "fork failed");

    if pid == 0 {
        let installed = match install_memory_backstop(16 * 1024 * 1024) {
            Some(bytes) => bytes,
            None => unsafe { libc::_exit(10) },
        };
        let mut now = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if unsafe { libc::getrlimit(libc::RLIMIT_DATA, &mut now) } != 0 {
            unsafe { libc::_exit(11) };
        }
        if now.rlim_cur == libc::RLIM_INFINITY || (now.rlim_cur as usize) > installed {
            unsafe { libc::_exit(12) };
        }
        let map = |bytes: usize| unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                bytes,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        };
        let within = 8 * 1024 * 1024;
        let healthy = map(within);
        if healthy == libc::MAP_FAILED {
            unsafe { libc::_exit(13) };
        }
        unsafe { std::ptr::write_bytes(healthy.cast::<u8>(), 0xA5, within) };
        if map(installed) != libc::MAP_FAILED {
            unsafe { libc::_exit(14) };
        }
        unsafe { libc::_exit(0) };
    }

    let mut status: libc::c_int = 0;
    let waited = unsafe { libc::waitpid(pid, &mut status, 0) };
    assert_eq!(waited, pid, "waitpid failed");
    assert!(
        libc::WIFEXITED(status),
        "child did not exit normally (status {status})"
    );
    let code = libc::WEXITSTATUS(status);
    assert_eq!(
        code, 0,
        "child must keep healthy growth within the RLIMIT_DATA budget and reject \
         a reservation past it (exit code {code})"
    );
}
