//! Integration tests for resource enforcement through the alloc path.
//!
//! These tests verify that the ResourceTracker is actually called during
//! heap allocation and that memory limits are enforced.

molt_runtime::declare_app_bootstrap!(molt_runtime::AppBootstrapProvider::Unavailable(
    "molt-runtime/resource_enforcement"
));

use molt_runtime::resource::{
    LimitedTracker, ResourceLimits, ResourceTracker, UnlimitedTracker,
    clear_global_tracker_factory, install_memory_backstop, memory_backstop_budget,
    parse_human_size, set_tracker, with_tracker,
};

unsafe extern "C" {
    /// The real runtime startup entrypoint that parses the resource env vars
    /// and installs the global tracker (and, when a memory cap is set, the
    /// OS memory backstop). Compiled binaries call this from runtime init.
    fn molt_runtime_init_resources();
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

/// Serialize env-mutating tests in this integration binary. The runtime's
/// internal `TEST_MUTEX` is not visible here, but these tests run in a separate
/// process from the unit tests, so an integration-local mutex is sufficient.
static ENV_GUARD: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn clear_all_resource_env() {
    for key in [
        "MOLT_MEMORY_LIMIT",
        "MOLT_RESOURCE_MAX_MEMORY",
        "MOLT_RESOURCE_MAX_DURATION_MS",
        "MOLT_RESOURCE_MAX_ALLOCATIONS",
        "MOLT_RESOURCE_MAX_RECURSION_DEPTH",
        "MOLT_RESOURCE_MAX_OPERATION_RESULT",
        "MOLT_RESOURCE_MAX_POW_RESULT",
        "MOLT_RESOURCE_MAX_REPEAT_RESULT",
        "MOLT_RESOURCE_MAX_SHIFT_RESULT",
        "MOLT_RESOURCE_MAX_STRING_RESULT",
    ] {
        unsafe { std::env::remove_var(key) };
    }
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
    let _g = ENV_GUARD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    clear_all_resource_env();
    clear_global_tracker_factory();
    set_tracker(Box::new(UnlimitedTracker));

    unsafe { std::env::set_var("MOLT_RESOURCE_MAX_MEMORY", "1048576") };
    unsafe { std::env::set_var("MOLT_RESOURCE_MAX_ALLOCATIONS", "2") };

    // The runtime-init C entrypoint parses both fields into one tracker.
    let _restore = OsBackstopRestore::capture();
    unsafe { molt_runtime_init_resources() };

    assert!(matches!(
        with_tracker(|t| t.on_grow(2 * 1024 * 1024)).unwrap_err(),
        molt_runtime::resource::ResourceError::Memory { .. }
    ));
    let allocations = with_tracker(|t| (0..3).map(|_| t.on_allocate(8)).collect::<Vec<_>>());
    assert!(allocations[..2].iter().all(Result::is_ok));
    assert!(matches!(
        allocations[2],
        Err(molt_runtime::resource::ResourceError::Allocation { .. })
    ));

    clear_all_resource_env();
    clear_global_tracker_factory();
    set_tracker(Box::new(UnlimitedTracker));
}

/// End-to-end demonstration: `MOLT_MEMORY_LIMIT=64M` set BEFORE runtime init
/// causes a >64MB allocation to be rejected by the in-VM tracker (Layer 1) —
/// the host is never OOM-ed. This is the exact path a compiled binary takes at
/// startup (`molt_runtime_init_resources` is the runtime-init C entrypoint).
#[test]
fn molt_memory_limit_alias_enforces_via_real_init_path() {
    let _g = ENV_GUARD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    clear_all_resource_env();
    clear_global_tracker_factory();
    set_tracker(Box::new(UnlimitedTracker));

    // Human-readable front door — resolves into ResourceLimits.max_memory.
    unsafe { std::env::set_var("MOLT_MEMORY_LIMIT", "64M") };

    // Run the actual runtime resource initialization (parses env, installs the
    // global LimitedTracker + OS memory backstop).
    let _restore = OsBackstopRestore::capture();
    unsafe { molt_runtime_init_resources() };

    // A small allocation under the cap succeeds.
    assert!(
        with_tracker(|t| t.on_grow(1024 * 1024)).is_ok(),
        "1 MiB should fit under the 64 MiB cap"
    );
    // A single allocation past the 64 MiB cap is rejected (logical Python heap
    // accounting) — NOT an OS OOM-kill of the test process.
    let over = 64 * 1024 * 1024 + 1;
    let err = with_tracker(|t| t.on_grow(over)).unwrap_err();
    assert!(
        matches!(err, molt_runtime::resource::ResourceError::Memory { .. }),
        "allocation past the cap must raise ResourceError::Memory, got {err:?}"
    );

    // Teardown so sibling tests and later processes start clean.
    clear_all_resource_env();
    clear_global_tracker_factory();
    set_tracker(Box::new(UnlimitedTracker));
}

/// Without any memory-limit env set, runtime init installs NO limit: a large
/// allocation succeeds (unchanged default behavior).
#[test]
fn no_memory_limit_env_means_unchanged_behavior() {
    let _g = ENV_GUARD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    clear_all_resource_env();
    clear_global_tracker_factory();
    set_tracker(Box::new(UnlimitedTracker));

    unsafe { molt_runtime_init_resources() };

    // No tracker installed -> the default UnlimitedTracker permits a large grow.
    assert!(
        with_tracker(|t| t.on_grow(256 * 1024 * 1024)).is_ok(),
        "with no limit configured, a 256 MiB grow must succeed (default behavior)"
    );

    clear_all_resource_env();
    clear_global_tracker_factory();
    set_tracker(Box::new(UnlimitedTracker));
}

/// The `MOLT_RESOURCE_MAX_MEMORY` canonical field and the `MOLT_MEMORY_LIMIT`
/// human-size alias resolve to the SAME limit (single enforcement path); the
/// alias wins when both are set.
#[test]
fn alias_and_canonical_field_share_one_enforcement_path() {
    let _g = ENV_GUARD
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    clear_all_resource_env();
    clear_global_tracker_factory();
    set_tracker(Box::new(UnlimitedTracker));

    // 1 MiB via human alias overrides a larger raw-byte canonical value.
    unsafe {
        std::env::set_var("MOLT_MEMORY_LIMIT", "1M");
        std::env::set_var("MOLT_RESOURCE_MAX_MEMORY", "536870912"); // 512 MiB
    }
    let _restore = OsBackstopRestore::capture();
    unsafe { molt_runtime_init_resources() };

    // The 1 MiB alias is the effective cap: a 2 MiB grow is rejected.
    let err = with_tracker(|t| t.on_grow(2 * 1024 * 1024)).unwrap_err();
    assert!(matches!(
        err,
        molt_runtime::resource::ResourceError::Memory { .. }
    ));

    clear_all_resource_env();
    clear_global_tracker_factory();
    set_tracker(Box::new(UnlimitedTracker));
}

/// The human-size parser used by the front door accepts the documented forms.
#[test]
fn human_size_front_door_parses_documented_forms() {
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
        installed.is_some_and(|bytes| bytes > memory_backstop_budget(1usize << 40)),
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
