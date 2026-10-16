use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use super::state::DaemonLru;

/// The daemon's compiled-output cache, shared by concurrent connections.
///
/// The LRU lock is held only to look up or insert bytes, never across a
/// compile. `begin` gives single-flight per key: the first caller compiles,
/// and concurrent callers for the same key wait, then reuse its output.
pub(crate) struct DaemonCache {
    lru: Mutex<DaemonLru>,
    max_bytes: Option<usize>,
    in_flight: Mutex<HashMap<String, Arc<InFlight>>>,
}

#[derive(Default)]
struct InFlight {
    done: Mutex<bool>,
    finished: Condvar,
    /// Callers blocked on this flight, so tests can order their threads.
    #[cfg(test)]
    waiting: std::sync::atomic::AtomicUsize,
}

/// A caller's place in one key's single flight.
pub(crate) enum Flight<'a> {
    /// This caller compiles; dropping the guard releases the waiters.
    Leader(FlightGuard<'a>),
    /// Another caller compiled this key while this one waited.
    Waited,
}

pub(crate) struct FlightGuard<'a> {
    cache: &'a DaemonCache,
    key: String,
    flight: Arc<InFlight>,
}

impl Drop for FlightGuard<'_> {
    fn drop(&mut self) {
        lock(&self.cache.in_flight).remove(&self.key);
        *lock(&self.flight.done) = true;
        self.flight.finished.notify_all();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // A panicking compile must not wedge every later request.
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl DaemonCache {
    pub(crate) fn new(max_bytes: Option<usize>) -> Self {
        Self {
            lru: Mutex::new(DaemonLru::new(max_bytes)),
            max_bytes,
            in_flight: Mutex::new(HashMap::new()),
        }
    }

    pub(crate) fn max_bytes(&self) -> Option<usize> {
        self.max_bytes
    }

    pub(crate) fn get_bytes(&self, key: &str) -> Option<Arc<[u8]>> {
        lock(&self.lru).get_bytes(key)
    }

    pub(crate) fn insert(&self, key: String, value: Arc<[u8]>) {
        lock(&self.lru).insert(key, value);
    }

    #[cfg(all(test, feature = "native-backend"))]
    pub(crate) fn is_empty(&self) -> bool {
        lock(&self.lru).entries.is_empty()
    }

    /// Entry count and resident bytes, for health reports.
    pub(crate) fn usage(&self) -> (usize, usize) {
        let lru = lock(&self.lru);
        (lru.entries.len(), lru.bytes)
    }

    /// Join `key`'s single flight.
    pub(crate) fn begin(&self, key: &str) -> Flight<'_> {
        let leader = {
            let mut in_flight = lock(&self.in_flight);
            match in_flight.get(key) {
                Some(existing) => Err(Arc::clone(existing)),
                None => {
                    let flight = Arc::new(InFlight::default());
                    in_flight.insert(key.to_string(), Arc::clone(&flight));
                    Ok(flight)
                }
            }
        };
        match leader {
            Ok(flight) => Flight::Leader(FlightGuard {
                cache: self,
                key: key.to_string(),
                flight,
            }),
            Err(existing) => {
                #[cfg(test)]
                existing
                    .waiting
                    .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let mut done = lock(&existing.done);
                while !*done {
                    done = existing
                        .finished
                        .wait(done)
                        .unwrap_or_else(PoisonError::into_inner);
                }
                Flight::Waited
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::Ordering;
    use std::thread;

    /// Blocks until `count` callers wait on `flight`.
    fn await_waiters(flight: &InFlight, count: usize) {
        while flight.waiting.load(Ordering::SeqCst) < count {
            thread::yield_now();
        }
    }

    #[test]
    fn one_caller_compiles_and_concurrent_callers_reuse_its_output() {
        let cache = Arc::new(DaemonCache::new(None));
        let Flight::Leader(guard) = cache.begin("module") else {
            panic!("the first caller leads");
        };
        let followers: Vec<_> = (0..4)
            .map(|_| {
                let cache = Arc::clone(&cache);
                thread::spawn(move || {
                    matches!(cache.begin("module"), Flight::Waited)
                        && cache.get_bytes("module").as_deref() == Some(b"out".as_slice())
                })
            })
            .collect();
        await_waiters(&guard.flight, 4);
        cache.insert("module".to_string(), Arc::from(b"out".as_slice()));
        drop(guard);
        for follower in followers {
            assert!(
                follower.join().expect("follower"),
                "a follower compiled again"
            );
        }
        // The flight ended, so the next caller for the key leads again.
        assert!(matches!(cache.begin("module"), Flight::Leader(_)));
    }

    #[test]
    fn a_leader_without_output_releases_its_followers() {
        let cache = Arc::new(DaemonCache::new(None));
        let Flight::Leader(guard) = cache.begin("module") else {
            panic!("the first caller leads");
        };
        let follower = {
            let cache = Arc::clone(&cache);
            thread::spawn(move || {
                matches!(cache.begin("module"), Flight::Waited)
                    && cache.get_bytes("module").is_none()
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
        let first = cache.begin("first");
        let second = cache.begin("second");
        assert!(matches!(first, Flight::Leader(_)));
        assert!(matches!(second, Flight::Leader(_)));
    }
}
