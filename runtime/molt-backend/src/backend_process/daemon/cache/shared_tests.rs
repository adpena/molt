use super::*;
use crate::backend_process::daemon::protocol::DaemonJobRequest;
use std::sync::atomic::Ordering;
use std::thread;

fn key(source: &str) -> Arc<DaemonCacheKey> {
    let mut job = DaemonJobRequest::from_json_value(
        &serde_json::json!({"id":"flight", "is_wasm":cfg!(feature="wasm-backend"),
            "output":"unused", "cache_key":source}),
        "job",
    )
    .expect("job");
    job.take_artifact_cache_keys()
        .expect("cache identity")
        .0
        .expect("nonempty key")
}

/// Blocks until `count` callers wait on `flight`.
fn await_waiters(flight: &InFlight, count: usize) {
    while flight.waiting.load(Ordering::SeqCst) < count {
        thread::yield_now();
    }
}

#[test]
fn one_caller_compiles_and_concurrent_callers_reuse_its_output() {
    let cache = Arc::new(DaemonCache::new(None));
    let key = key("module");
    let Flight::Leader(guard) = cache.begin(&key) else {
        panic!("the first caller leads");
    };
    let followers: Vec<_> = (0..4)
        .map(|_| {
            let cache = Arc::clone(&cache);
            let key = Arc::clone(&key);
            thread::spawn(move || {
                matches!(cache.begin(&key), Flight::Waited)
                    && cache.get_bytes(&key).as_deref() == Some(b"out".as_slice())
            })
        })
        .collect();
    await_waiters(&guard.flight, 4);
    cache.insert(Arc::clone(&key), Arc::from(b"out".as_slice()));
    drop(guard);
    for follower in followers {
        assert!(
            follower.join().expect("follower"),
            "a follower compiled again"
        );
    }
    // The flight ended, so the next caller for the key leads again.
    assert!(matches!(cache.begin(&key), Flight::Leader(_)));
}

#[test]
fn a_leader_without_output_releases_its_followers() {
    let cache = Arc::new(DaemonCache::new(None));
    let key = key("module");
    let Flight::Leader(guard) = cache.begin(&key) else {
        panic!("the first caller leads");
    };
    let follower = {
        let cache = Arc::clone(&cache);
        let key = Arc::clone(&key);
        thread::spawn(move || {
            matches!(cache.begin(&key), Flight::Waited) && cache.get_bytes(&key).is_none()
        })
    };
    await_waiters(&guard.flight, 1);
    // A failed or oversized compile caches nothing; the follower then
    // compiles for itself instead of waiting forever.
    drop(guard);
    assert!(follower.join().expect("follower"));
}

#[test]
fn flights_for_different_keys_do_not_block_each_other() {
    let cache = DaemonCache::new(None);
    let first = cache.begin(&key("first"));
    let second = cache.begin(&key("second"));
    assert!(matches!(first, Flight::Leader(_)));
    assert!(matches!(second, Flight::Leader(_)));
}

#[test]
fn same_source_different_target_has_independent_flight_state() {
    let cache = DaemonCache::new(None);
    let first = key("same");
    let mut changed = DaemonJobRequest::from_json_value(
        &serde_json::json!({"id":"flight", "is_wasm":cfg!(feature="wasm-backend"),
            "output":"unused", "cache_key":"same",
            "wasm_data_base":4096, "target_triple":"aarch64-unknown-linux-gnu"}),
        "job",
    )
    .expect("changed target");
    let changed = changed
        .take_artifact_cache_keys()
        .expect("cache identity")
        .0
        .unwrap();
    let Flight::Leader(first_guard) = cache.begin(&first) else {
        panic!("first target must lead");
    };
    // Inspect the lookup before joining, so a broken key does not deadlock the oracle.
    assert!(!lock(&cache.in_flight).contains_key(changed.as_ref()));
    let Flight::Leader(changed_guard) = cache.begin(&changed) else {
        panic!("different target must lead independently");
    };
    assert_eq!(lock(&cache.in_flight).len(), 2);
    drop(first_guard);
    assert!(lock(&cache.in_flight).contains_key(changed.as_ref()));
    drop(changed_guard);
    assert!(lock(&cache.in_flight).is_empty());
}

#[test]
fn same_source_different_actual_semantics_has_independent_flight_state() {
    use molt_backend::backend_environment::{BackendEnvironment, enter};
    let scoped_key = |value: &str| {
        let environment = BackendEnvironment::from_request(
            serde_json::json!({"MOLT_DISABLE_RC_COALESCE":value})
                .as_object()
                .unwrap(),
        )
        .unwrap();
        let _scope = enter(Arc::new(environment));
        key("same")
    };
    let cache = DaemonCache::new(None);
    let first = scoped_key("");
    let second = scoped_key("1");
    let Flight::Leader(first_guard) = cache.begin(&first) else {
        panic!("first semantic input must lead");
    };
    // A lookup assertion before begin prevents a broken key from deadlocking
    // this test while the first leader is deliberately retained.
    assert!(!lock(&cache.in_flight).contains_key(second.as_ref()));
    let Flight::Leader(second_guard) = cache.begin(&second) else {
        panic!("different actual semantics must lead independently");
    };
    assert_eq!(lock(&cache.in_flight).len(), 2);
    cache.insert(Arc::clone(&first), Arc::from(b"first".as_slice()));
    assert!(cache.get_bytes(&second).is_none());
    drop(first_guard);
    assert!(lock(&cache.in_flight).contains_key(second.as_ref()));
    drop(second_guard);
    assert!(lock(&cache.in_flight).is_empty());
}
