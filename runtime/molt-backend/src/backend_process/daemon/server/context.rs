use std::io::{self, Write};
use std::os::unix::fs::MetadataExt;
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex, MutexGuard, PoisonError};
use std::time::{Duration, Instant};

use super::super::super::config::BACKEND_DAEMON_PROTOCOL_VERSION;
use super::super::cache::{DaemonCache, DaemonStats};
use super::super::protocol::DaemonHealthResponse;

/// Everything concurrent connections share in one daemon process.
pub(crate) struct DaemonState {
    pub(crate) cache: DaemonCache,
    pub(crate) stats: Mutex<DaemonStats>,
    pub(crate) spawn_config_digest: Option<String>,
    /// The most recent request's configuration digest, for health reports.
    pub(crate) last_config_digest: Mutex<Option<String>>,
    pub(crate) started_at: Instant,
    pub(crate) request_limit_bytes: usize,
    pub(crate) max_jobs: usize,
    compile_slots: CompileSlots,
    /// Jobs admitted and not yet answered.
    active_jobs: AtomicUsize,
    connections: Counter,
    retirement: Retirement,
}

/// At most one compile per core runs at once; the rest wait for a slot.
struct CompileSlots {
    free: Mutex<usize>,
    released: Condvar,
}

pub(crate) struct CompileSlot<'a>(&'a CompileSlots);

impl Drop for CompileSlot<'_> {
    fn drop(&mut self) {
        *lock(&self.0.free) += 1;
        self.0.released.notify_one();
    }
}

/// Live connection threads; the daemon exits only after the last one.
struct Counter {
    live: Mutex<Live>,
    drained: Condvar,
}

struct Live {
    count: usize,
    /// When the count last fell to zero, or the daemon started.
    idle_since: Instant,
}

/// Holds one live connection until dropped.
pub(crate) struct Connection(std::sync::Arc<DaemonState>);

impl Drop for Connection {
    fn drop(&mut self) {
        let mut live = lock(&self.0.connections.live);
        live.count -= 1;
        if live.count == 0 {
            live.idle_since = Instant::now();
            self.0.connections.drained.notify_all();
        }
    }
}

/// Counts admitted jobs until dropped.
pub(crate) struct ActiveJobs<'a> {
    state: &'a DaemonState,
    count: usize,
}

impl Drop for ActiveJobs<'_> {
    fn drop(&mut self) {
        self.state
            .active_jobs
            .fetch_sub(self.count, Ordering::AcqRel);
    }
}

/// Retires the daemon on request: the socket disappears first, so a new
/// build starts a fresh daemon, and work already accepted finishes.
pub(crate) struct Retirement {
    socket_path: PathBuf,
    /// The (device, inode) of the socket this daemon bound. A successor may
    /// bind the same path once this daemon stops owning it.
    bound: (u64, u64),
    wake: Mutex<UnixStream>,
    started: AtomicBool,
}

impl Retirement {
    /// Call right after binding ``socket_path``.
    pub(crate) fn new(socket_path: PathBuf, wake: UnixStream) -> io::Result<Self> {
        let bound = std::fs::symlink_metadata(&socket_path)?;
        Ok(Self {
            socket_path,
            bound: (bound.dev(), bound.ino()),
            wake: Mutex::new(wake),
            started: AtomicBool::new(false),
        })
    }

    /// Unlinks the socket only while it is still the one this daemon bound.
    fn unlink_own_socket(&self) {
        let path = &self.socket_path;
        match std::fs::symlink_metadata(path) {
            Ok(meta) if (meta.dev(), meta.ino()) == self.bound => {
                if let Err(err) = std::fs::remove_file(path)
                    && err.kind() != io::ErrorKind::NotFound
                {
                    eprintln!("backend daemon retirement could not remove its socket: {err}");
                }
            }
            // A successor's socket, or none: nothing of this daemon's remains.
            Ok(_) => {}
            Err(err) if err.kind() == io::ErrorKind::NotFound => {}
            Err(err) => eprintln!("backend daemon retirement could not inspect its socket: {err}"),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

impl DaemonState {
    pub(crate) fn new(
        cache: DaemonCache,
        spawn_config_digest: Option<String>,
        request_limit_bytes: usize,
        max_jobs: usize,
        compile_slots: usize,
        retirement: Retirement,
    ) -> Self {
        Self {
            cache,
            stats: Mutex::new(DaemonStats::default()),
            spawn_config_digest,
            last_config_digest: Mutex::new(None),
            started_at: Instant::now(),
            request_limit_bytes,
            max_jobs,
            compile_slots: CompileSlots {
                free: Mutex::new(compile_slots.max(1)),
                released: Condvar::new(),
            },
            active_jobs: AtomicUsize::new(0),
            connections: Counter {
                live: Mutex::new(Live {
                    count: 0,
                    idle_since: Instant::now(),
                }),
                drained: Condvar::new(),
            },
            retirement,
        }
    }

    pub(crate) fn compile_slot(&self) -> CompileSlot<'_> {
        let mut free = lock(&self.compile_slots.free);
        while *free == 0 {
            free = self
                .compile_slots
                .released
                .wait(free)
                .unwrap_or_else(PoisonError::into_inner);
        }
        *free -= 1;
        CompileSlot(&self.compile_slots)
    }

    /// Registers a connection thread the daemon must outlive.
    pub(crate) fn connection(self: &std::sync::Arc<Self>) -> Connection {
        lock(&self.connections.live).count += 1;
        Connection(std::sync::Arc::clone(self))
    }

    /// How long the daemon has served no connection; `None` while one is live.
    pub(crate) fn idle_for(&self) -> Option<Duration> {
        let live = lock(&self.connections.live);
        (live.count == 0).then(|| live.idle_since.elapsed())
    }

    pub(crate) fn wait_for_connections(&self) {
        let mut live = lock(&self.connections.live);
        while live.count > 0 {
            live = self
                .connections
                .drained
                .wait(live)
                .unwrap_or_else(PoisonError::into_inner);
        }
    }

    pub(crate) fn admit_jobs(&self, count: usize) -> ActiveJobs<'_> {
        self.active_jobs.fetch_add(count, Ordering::AcqRel);
        ActiveJobs { state: self, count }
    }

    pub(crate) fn retiring(&self) -> bool {
        self.retirement.started.load(Ordering::Acquire)
    }

    /// Stops new connections and wakes the accept loop to drain and exit.
    pub(crate) fn retire(&self) {
        if self.retirement.started.swap(true, Ordering::AcqRel) {
            return;
        }
        // Unlink before waking: a client that connects later finds no socket
        // and starts a fresh daemon instead of queueing on this one.
        self.retirement.unlink_own_socket();
        if let Err(err) = lock(&self.retirement.wake).write_all(&[1]) {
            eprintln!("backend daemon retirement could not wake its listener: {err}");
        }
    }

    pub(crate) fn stats(&self) -> MutexGuard<'_, DaemonStats> {
        lock(&self.stats)
    }

    pub(crate) fn record_config_digest(&self, digest: Option<&str>) {
        if let Some(digest) = digest {
            *lock(&self.last_config_digest) = Some(digest.to_string());
        }
    }

    pub(crate) fn health(&self) -> DaemonHealthResponse {
        let stats = self.stats();
        let (cache_entries, cache_bytes) = self.cache.usage();
        DaemonHealthResponse {
            protocol_version: BACKEND_DAEMON_PROTOCOL_VERSION,
            pid: std::process::id(),
            spawn_config_digest: self.spawn_config_digest.clone(),
            active_config_digest: lock(&self.last_config_digest).clone(),
            uptime_ms: self
                .started_at
                .elapsed()
                .as_millis()
                .min(u128::from(u64::MAX)) as u64,
            cache_entries,
            cache_bytes,
            cache_max_bytes: self.cache.max_bytes(),
            request_limit_bytes: Some(self.request_limit_bytes),
            max_jobs: Some(self.max_jobs),
            requests_total: stats.requests_total,
            jobs_total: stats.jobs_total,
            cache_hits: stats.cache_hits,
            cache_misses: stats.cache_misses,
            active_jobs: self.active_jobs.load(Ordering::Acquire),
            retiring: self.retiring(),
        }
    }
}
