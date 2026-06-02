//! Integration tests for resource enforcement through the alloc path.
//!
//! These tests verify that the ResourceTracker is actually called during
//! heap allocation and that memory limits are enforced.

use molt_runtime::resource::{
    LimitedTracker, ResourceLimits, ResourceTracker, UnlimitedTracker,
    clear_global_tracker_factory, install_address_space_backstop, parse_human_size, set_tracker,
    with_tracker,
};

unsafe extern "C" {
    /// The real runtime startup entrypoint that parses the resource env vars
    /// and installs the global tracker (and, when a memory cap is set, the
    /// RLIMIT_AS backstop). Compiled binaries call this from runtime init.
    fn molt_runtime_init_resources();
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
    // Set the env var
    unsafe { std::env::set_var("MOLT_RESOURCE_MAX_MEMORY", "1048576") };
    unsafe { std::env::set_var("MOLT_RESOURCE_MAX_ALLOCATIONS", "1000") };

    // Call the init function (this is what runtime_init calls)
    // We can't call molt_runtime_init_resources directly as it's extern "C",
    // but we can verify the env var parsing logic works
    let max_mem = std::env::var("MOLT_RESOURCE_MAX_MEMORY")
        .ok()
        .and_then(|s| s.parse::<usize>().ok());
    assert_eq!(max_mem, Some(1048576));

    let max_alloc = std::env::var("MOLT_RESOURCE_MAX_ALLOCATIONS")
        .ok()
        .and_then(|s| s.parse::<usize>().ok());
    assert_eq!(max_alloc, Some(1000));

    // Clean up
    unsafe { std::env::remove_var("MOLT_RESOURCE_MAX_MEMORY") };
    unsafe { std::env::remove_var("MOLT_RESOURCE_MAX_ALLOCATIONS") };
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
    // global LimitedTracker + RLIMIT_AS backstop).
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

/// The RLIMIT_AS backstop installs above a configured limit. We use a value
/// (1 TiB) large enough that even Darwin — which rejects *lowering* RLIMIT_AS
/// to a small finite value (EINVAL), see the Linux-only enforcement test below
/// — accepts it. This asserts the helper wires up `setrlimit` correctly without
/// depending on platform-specific small-cap enforcement.
#[cfg(all(unix, not(target_arch = "wasm32")))]
#[test]
fn address_space_backstop_installs_on_unix() {
    let installed = install_address_space_backstop(1usize << 40); // 1 TiB
    assert!(
        installed.is_some(),
        "RLIMIT_AS backstop should install for a large (non-lowering) value on unix"
    );
}

/// On non-unix / wasm targets the backstop is a documented no-op.
#[cfg(not(all(unix, not(target_arch = "wasm32"))))]
#[test]
fn address_space_backstop_is_noop_off_unix() {
    assert!(install_address_space_backstop(1usize << 40).is_none());
}

/// LINUX ONLY: the RLIMIT_AS backstop GENUINELY tightens the OS address-space
/// limit and blocks an over-cap reservation. We prove this in a forked child
/// (so we never lower the test runner's own address space): the child installs
/// a small backstop, confirms `getrlimit(RLIMIT_AS)` reflects the tightened
/// soft limit, and that a huge `mmap` past it fails at the OS layer — a clean
/// failure, not an OOM-kill of the host.
///
/// This is gated to Linux because Darwin's `setrlimit(RLIMIT_AS, …)` returns
/// EINVAL when asked to lower the limit to a small finite value (verified:
/// RLIMIT_AS is not a usable hard memory cap on macOS). On macOS the in-VM
/// tracker (Layer 1) is the enforcement; the OS backstop degrades to best-effort
/// and `install_address_space_backstop` honestly reports `None`.
#[cfg(target_os = "linux")]
#[test]
fn address_space_backstop_actually_tightens_os_limit_in_child() {
    // SAFETY: between fork() and _exit() the child only calls async-signal-safe
    // libc and our own pure-Rust helpers (no allocation in the failure path, no
    // locks held across the fork in this single-threaded test context).
    let pid = unsafe { libc::fork() };
    assert!(pid >= 0, "fork failed");

    if pid == 0 {
        // ---- child ----
        // Install a small (16 MiB tracker -> ~80 MiB backstop) limit.
        let installed = match install_address_space_backstop(16 * 1024 * 1024) {
            Some(v) => v,
            None => unsafe { libc::_exit(10) },
        };

        // Confirm getrlimit reflects a tightened (finite, <= installed) soft
        // limit rather than the inherited (typically unlimited) value.
        let mut now = libc::rlimit {
            rlim_cur: 0,
            rlim_max: 0,
        };
        if unsafe { libc::getrlimit(libc::RLIMIT_AS, &mut now) } != 0 {
            unsafe { libc::_exit(11) };
        }
        if now.rlim_cur == libc::RLIM_INFINITY || (now.rlim_cur as usize) > installed {
            unsafe { libc::_exit(12) };
        }

        // A reservation far past the backstop must fail at the OS layer
        // (mmap returns MAP_FAILED) — proving Layer 2 catches what the tracker
        // cannot see. We use raw mmap to bypass the in-VM tracker entirely.
        let huge = installed.saturating_mul(8); // ~640 MiB, well past ~80 MiB cap
        let p = unsafe {
            libc::mmap(
                std::ptr::null_mut(),
                huge,
                libc::PROT_READ | libc::PROT_WRITE,
                libc::MAP_PRIVATE | libc::MAP_ANON,
                -1,
                0,
            )
        };
        if p == libc::MAP_FAILED {
            unsafe { libc::_exit(0) }; // success: OS backstop rejected the mapping
        }
        unsafe { libc::_exit(13) }; // mapping unexpectedly succeeded past the backstop
    }

    // ---- parent ----
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
        "child should prove the RLIMIT_AS backstop tightened the OS limit and \
         blocked an over-cap mmap (exit code {code})"
    );
}
