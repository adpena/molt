//! Minimal connection pool skeleton for Molt DB integrations.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

struct PoolState<T> {
    idle: Vec<T>,
}

/// A bounded pool for reusable connection-like objects.
pub struct Pool<T> {
    max: usize,
    factory: Box<dyn Fn() -> T + Send + Sync>,
    state: Mutex<PoolState<T>>,
    available: Condvar,
    in_flight: AtomicUsize,
}

/// A pooled value that returns to the pool on drop.
pub struct Pooled<T> {
    pool: Arc<Pool<T>>,
    value: Option<T>,
}

impl<T> Pool<T> {
    pub fn new<F>(max: usize, factory: F) -> Arc<Self>
    where
        F: Fn() -> T + Send + Sync + 'static,
    {
        Arc::new(Self {
            max: max.max(1),
            factory: Box::new(factory),
            state: Mutex::new(PoolState { idle: Vec::new() }),
            available: Condvar::new(),
            in_flight: AtomicUsize::new(0),
        })
    }

    pub fn acquire(self: &Arc<Self>, timeout: Option<Duration>) -> Option<Pooled<T>> {
        let deadline = timeout.map(|limit| Instant::now() + limit);
        loop {
            let mut state = self.state.lock().unwrap();
            if let Some(item) = state.idle.pop() {
                return Some(Pooled {
                    pool: Arc::clone(self),
                    value: Some(item),
                });
            }

            if self.in_flight.load(Ordering::SeqCst) < self.max {
                self.in_flight.fetch_add(1, Ordering::SeqCst);
                drop(state);
                let item = (self.factory)();
                return Some(Pooled {
                    pool: Arc::clone(self),
                    value: Some(item),
                });
            }

            match deadline {
                None => {
                    state = self.available.wait(state).unwrap();
                }
                Some(limit) => {
                    let now = Instant::now();
                    if now >= limit {
                        return None;
                    }
                    let remaining = limit - now;
                    let (guard, _) = self.available.wait_timeout(state, remaining).unwrap();
                    state = guard;
                    if Instant::now() >= limit {
                        return None;
                    }
                }
            }
        }
    }

    pub fn in_flight(&self) -> usize {
        self.in_flight.load(Ordering::SeqCst)
    }

    pub fn idle_count(&self) -> usize {
        let state = self.state.lock().unwrap();
        state.idle.len()
    }
}

impl<T> Pooled<T> {
    pub fn as_ref(&self) -> &T {
        self.value
            .as_ref()
            .expect("Pooled value missing (already released)")
    }

    pub fn into_inner(mut self) -> T {
        self.value
            .take()
            .expect("Pooled value missing (already released)")
    }
}

impl<T> Drop for Pooled<T> {
    fn drop(&mut self) {
        if let Some(item) = self.value.take() {
            let mut state = self.pool.state.lock().unwrap();
            state.idle.push(item);
            self.pool.available.notify_one();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pool_reuses_connections() {
        let pool = Pool::new(2, || 7usize);
        let first = pool.acquire(None).expect("first");
        let second = pool.acquire(None).expect("second");
        assert_eq!(pool.in_flight(), 2);
        drop(first);
        assert_eq!(pool.idle_count(), 1);
        drop(second);
        assert_eq!(pool.idle_count(), 2);
    }

    #[test]
    fn pool_timeout() {
        let pool = Pool::new(1, || 42usize);
        let _guard = pool.acquire(None).expect("guard");
        let result = pool.acquire(Some(Duration::from_millis(10)));
        assert!(result.is_none());
    }
}
