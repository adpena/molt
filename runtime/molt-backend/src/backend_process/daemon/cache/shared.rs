use std::collections::HashMap;
use std::sync::{Arc, Condvar, Mutex, MutexGuard, PoisonError};

use molt_ir::FunctionIR;
use molt_ir::addressed_program::FunctionDigest;

use super::super::protocol::DaemonCacheKey;
use super::state::DaemonLru;

/// The daemon's compiled-output cache, shared by concurrent connections.
///
/// The LRU lock is held only to look up or insert bytes, never across a
/// compile. `begin` gives single-flight per key: the first caller compiles,
/// and concurrent callers for the same key wait, then reuse its output.
pub(crate) struct DaemonCache {
    lru: Mutex<DaemonLru>,
    max_bytes: Option<usize>,
    in_flight: Mutex<HashMap<Arc<DaemonCacheKey>, Arc<InFlight>>>,
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
    key: Arc<DaemonCacheKey>,
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

    pub(crate) fn get_bytes(&self, key: &Arc<DaemonCacheKey>) -> Option<Arc<[u8]>> {
        lock(&self.lru).get_bytes(key)
    }

    pub(crate) fn insert(&self, key: Arc<DaemonCacheKey>, value: Arc<[u8]>) {
        lock(&self.lru).insert(key, value);
    }

    /// Program positions whose bodies are not resident. One lock covers the
    /// whole program; each resident body is promoted.
    pub(crate) fn missing_functions(&self, digests: &[FunctionDigest]) -> Vec<usize> {
        let mut lru = lock(&self.lru);
        digests
            .iter()
            .enumerate()
            .filter(|(_, digest)| lru.get_function(**digest).is_none())
            .map(|(position, _)| position)
            .collect()
    }

    /// The resident body of each position `wanted` selects, under one lock.
    pub(crate) fn resident_functions(
        &self,
        digests: &[FunctionDigest],
        wanted: impl Fn(&FunctionDigest) -> bool,
    ) -> Vec<Option<Arc<FunctionIR>>> {
        let mut lru = lock(&self.lru);
        digests
            .iter()
            .map(|digest| wanted(digest).then(|| lru.get_function(*digest)).flatten())
            .collect()
    }

    /// Keep admitted bodies, under one lock.
    pub(crate) fn insert_functions(
        &self,
        functions: impl IntoIterator<Item = (FunctionDigest, Arc<FunctionIR>)>,
    ) {
        let mut lru = lock(&self.lru);
        for (digest, function) in functions {
            lru.insert_function(digest, function);
        }
    }

    #[cfg(all(test, feature = "native-backend"))]
    pub(crate) fn is_empty(&self) -> bool {
        lock(&self.lru).usage().0 == 0
    }

    /// Entry count and charged retained ownership bytes, for health reports.
    /// Includes keys/context/output storage; this is not process RSS.
    pub(crate) fn usage(&self) -> (usize, usize) {
        lock(&self.lru).usage()
    }

    /// Resident function bodies, counted by a linear scan.
    pub(crate) fn function_entries(&self) -> usize {
        lock(&self.lru).function_entries()
    }

    /// Resident compiled artifacts, counted by a linear scan.
    #[cfg(test)]
    pub(crate) fn artifact_entries(&self) -> usize {
        let lru = lock(&self.lru);
        lru.usage().0 - lru.function_entries()
    }

    /// Join `key`'s single flight.
    pub(crate) fn begin(&self, key: &Arc<DaemonCacheKey>) -> Flight<'_> {
        let leader = {
            let mut in_flight = lock(&self.in_flight);
            match in_flight.get(key.as_ref()) {
                Some(existing) => Err(Arc::clone(existing)),
                None => {
                    let flight = Arc::new(InFlight::default());
                    in_flight.insert(Arc::clone(key), Arc::clone(&flight));
                    Ok(flight)
                }
            }
        };
        match leader {
            Ok(flight) => Flight::Leader(FlightGuard {
                cache: self,
                key: Arc::clone(key),
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
#[path = "shared_tests.rs"]
mod tests;
