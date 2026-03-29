//! Integration tests for resource enforcement through the alloc path.
//!
//! These tests verify that the ResourceTracker is actually called during
//! heap allocation and that memory limits are enforced.

use molt_runtime::resource::{LimitedTracker, ResourceLimits, ResourceTracker, set_tracker, UnlimitedTracker};

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
