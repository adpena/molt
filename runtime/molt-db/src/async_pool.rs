//! Async connection pool primitives for Molt DB integrations.

use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::Notify;
use tokio::time::{Instant, sleep};

struct AsyncPoolState<T> {
    idle: Vec<T>,
}

pub type FactoryFuture<T> = Pin<Box<dyn Future<Output = Result<T, String>> + Send>>;

/// An async bounded pool for reusable connection-like objects.
pub struct AsyncPool<T> {
    max: usize,
    factory: Box<dyn Fn() -> FactoryFuture<T> + Send + Sync>,
    state: Mutex<AsyncPoolState<T>>,
    available: Notify,
    in_flight: AtomicUsize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AsyncAcquireError {
    Timeout,
    Cancelled,
    Create(String),
}

#[derive(Clone)]
pub struct CancelToken {
    inner: Arc<CancelState>,
}

struct CancelState {
    cancelled: AtomicBool,
    notify: Notify,
}

impl CancelToken {
    pub fn new() -> Self {
        Self {
            inner: Arc::new(CancelState {
                cancelled: AtomicBool::new(false),
                notify: Notify::new(),
            }),
        }
    }

    pub fn cancel(&self) {
        if !self.inner.cancelled.swap(true, Ordering::SeqCst) {
            self.inner.notify.notify_waiters();
        }
    }

    pub fn is_cancelled(&self) -> bool {
        self.inner.cancelled.load(Ordering::SeqCst)
    }

    pub async fn cancelled(&self) {
        if self.is_cancelled() {
            return;
        }
        self.inner.notify.notified().await;
    }
}

impl Default for CancelToken {
    fn default() -> Self {
        Self::new()
    }
}

/// A pooled async value that returns to the pool on drop.
pub struct AsyncPooled<T> {
    pool: Arc<AsyncPool<T>>,
    value: Option<T>,
}

impl<T> AsyncPool<T> {
    pub fn new<F, Fut, E>(max: usize, factory: F) -> Arc<Self>
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<T, E>> + Send + 'static,
        E: std::fmt::Display,
    {
        let factory = Box::new(move || {
            let fut = factory();
            Box::pin(async move { fut.await.map_err(|err| err.to_string()) }) as FactoryFuture<T>
        });
        Arc::new(Self {
            max: max.max(1),
            factory,
            state: Mutex::new(AsyncPoolState { idle: Vec::new() }),
            available: Notify::new(),
            in_flight: AtomicUsize::new(0),
        })
    }

    pub async fn acquire(
        self: &Arc<Self>,
        timeout: Option<Duration>,
        cancel: Option<&CancelToken>,
    ) -> Result<AsyncPooled<T>, AsyncAcquireError> {
        let deadline = timeout.map(|limit| Instant::now() + limit);
        loop {
            if let Some(token) = cancel {
                if token.is_cancelled() {
                    return Err(AsyncAcquireError::Cancelled);
                }
            }
            let item = {
                let mut state = self.state.lock().unwrap();
                state.idle.pop()
            };
            if let Some(item) = item {
                return Ok(AsyncPooled {
                    pool: Arc::clone(self),
                    value: Some(item),
                });
            }
            if self.in_flight.load(Ordering::SeqCst) < self.max {
                self.in_flight.fetch_add(1, Ordering::SeqCst);
                match (self.factory)().await {
                    Ok(item) => {
                        return Ok(AsyncPooled {
                            pool: Arc::clone(self),
                            value: Some(item),
                        });
                    }
                    Err(err) => {
                        self.in_flight.fetch_sub(1, Ordering::SeqCst);
                        self.available.notify_one();
                        return Err(AsyncAcquireError::Create(err));
                    }
                }
            }

            let wait = match deadline {
                None => None,
                Some(limit) => {
                    let now = Instant::now();
                    if now >= limit {
                        return Err(AsyncAcquireError::Timeout);
                    }
                    Some(limit - now)
                }
            };

            match (cancel, wait) {
                (Some(token), Some(duration)) => {
                    tokio::select! {
                        _ = self.available.notified() => {},
                        _ = token.cancelled() => return Err(AsyncAcquireError::Cancelled),
                        _ = sleep(duration) => return Err(AsyncAcquireError::Timeout),
                    }
                }
                (Some(token), None) => {
                    tokio::select! {
                        _ = self.available.notified() => {},
                        _ = token.cancelled() => return Err(AsyncAcquireError::Cancelled),
                    }
                }
                (None, Some(duration)) => {
                    tokio::select! {
                        _ = self.available.notified() => {},
                        _ = sleep(duration) => return Err(AsyncAcquireError::Timeout),
                    }
                }
                (None, None) => {
                    self.available.notified().await;
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

    fn release(&self, item: T) {
        let mut state = self.state.lock().unwrap();
        state.idle.push(item);
        self.available.notify_one();
    }

    fn discard(&self) {
        self.in_flight.fetch_sub(1, Ordering::SeqCst);
        self.available.notify_one();
    }
}

impl<T> AsyncPooled<T> {
    pub fn into_inner(mut self) -> T {
        self.value
            .take()
            .expect("AsyncPooled value missing (already released)")
    }

    pub fn discard(mut self) {
        if self.value.take().is_some() {
            self.pool.discard();
        }
    }
}

impl<T> AsRef<T> for AsyncPooled<T> {
    fn as_ref(&self) -> &T {
        self.value
            .as_ref()
            .expect("AsyncPooled value missing (already released)")
    }
}

impl<T> Drop for AsyncPooled<T> {
    fn drop(&mut self) {
        if let Some(item) = self.value.take() {
            self.pool.release(item);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn async_pool_reuses_connections() {
        let pool = AsyncPool::new(2, || async { Ok::<_, std::io::Error>(7usize) });
        let first = pool.acquire(None, None).await.expect("first");
        let second = pool.acquire(None, None).await.expect("second");
        assert_eq!(pool.in_flight(), 2);
        drop(first);
        assert_eq!(pool.idle_count(), 1);
        drop(second);
        assert_eq!(pool.idle_count(), 2);
    }

    #[tokio::test]
    async fn async_pool_timeout() {
        let pool = AsyncPool::new(1, || async { Ok::<_, std::io::Error>(42usize) });
        let _guard = pool.acquire(None, None).await.expect("guard");
        let result = pool.acquire(Some(Duration::from_millis(5)), None).await;
        assert_eq!(result.err(), Some(AsyncAcquireError::Timeout));
    }

    #[tokio::test]
    async fn async_pool_cancelled() {
        let pool = AsyncPool::new(1, || async { Ok::<_, std::io::Error>(7usize) });
        let _guard = pool.acquire(None, None).await.expect("guard");
        let token = CancelToken::new();
        token.cancel();
        let result = pool.acquire(None, Some(&token)).await;
        assert_eq!(result.err(), Some(AsyncAcquireError::Cancelled));
    }

    #[tokio::test]
    async fn async_pool_discard_allows_recreate() {
        let pool = AsyncPool::new(1, || async { Ok::<_, std::io::Error>(7usize) });
        let guard = pool.acquire(None, None).await.expect("guard");
        guard.discard();
        let next = pool.acquire(Some(Duration::from_millis(10)), None).await;
        assert!(next.is_ok());
    }
}
