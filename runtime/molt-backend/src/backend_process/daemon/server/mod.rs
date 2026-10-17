use std::io;

mod connection;
mod context;
mod dispatch;
mod responses;
mod wire;

use std::env;
use std::os::fd::AsRawFd;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use super::super::io_limits::{daemon_idle_timeout, daemon_max_jobs, daemon_request_limit_bytes};
use super::super::memory_guard::widen_process_memory_guard;
use super::cache::{DaemonCache, daemon_cache_limit_bytes};
use connection::handle_daemon_connection;
use molt_backend::tir::pipeline_cache::COMPILE_THREAD_STACK_BYTES;

const ACCEPT_ERROR_BACKOFF: std::time::Duration = std::time::Duration::from_millis(100);
use context::{DaemonState, Retirement};

#[expect(
    clippy::disallowed_methods,
    reason = "the spawning build fixes the daemon's configuration digest"
)]
fn spawn_config_digest() -> Option<String> {
    env::var("MOLT_BACKEND_DAEMON_CONFIG_DIGEST")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

pub(crate) fn run_daemon(socket_path: &str) -> io::Result<()> {
    let socket = Path::new(socket_path);
    if socket.exists() {
        let _ = std::fs::remove_file(socket);
    }
    if let Some(parent) = socket.parent()
        && !parent.as_os_str().is_empty()
    {
        std::fs::create_dir_all(parent)?;
    }

    let listener = UnixListener::bind(socket)?;
    listener.set_nonblocking(true)?;
    let (wake, woken) = UnixStream::pair()?;
    let compile_slots = std::thread::available_parallelism().map_or(1, usize::from);
    // Read while the spawning build's budget is still in the environment.
    widen_process_memory_guard(compile_slots);
    // SAFETY: the daemon has not spawned a thread yet.
    unsafe { molt_backend::backend_environment::serve_requests() };
    let state = Arc::new(DaemonState::new(
        DaemonCache::new(Some(daemon_cache_limit_bytes())),
        spawn_config_digest(),
        daemon_request_limit_bytes(),
        daemon_max_jobs(),
        compile_slots,
        Retirement::new(socket.to_path_buf(), wake)?,
    ));
    serve(&listener, &woken, &state, daemon_idle_timeout())
}

/// Accepts connections until the daemon retires: on request, or once no
/// connection has been live for ``idle_timeout``.
fn serve(
    listener: &UnixListener,
    woken: &UnixStream,
    state: &Arc<DaemonState>,
    idle_timeout: Option<Duration>,
) -> io::Result<()> {
    // Concurrent builds share one warm daemon: each connection is served on
    // its own thread, and compiles beyond the core count wait for a slot.
    let mut wait = idle_timeout;
    loop {
        match wait_for_connection_or_retirement(listener, woken, wait)? {
            Wake::Idle => {
                // A daemon nobody uses must not outlive its builds. The idle
                // clock starts when the last connection ends, so a long
                // compile never shortens the next idle period.
                let timeout = idle_timeout.expect("only a timed wait reports idle");
                match state.idle_for() {
                    // Retiring unlinks the socket and wakes this loop, which
                    // drains any client that connected meanwhile.
                    Some(idle) if idle >= timeout => state.retire(),
                    Some(idle) => wait = Some(timeout - idle),
                    None => wait = idle_timeout,
                }
            }
            Wake::Connection => {
                accept_queued(listener, state);
                wait = idle_timeout;
            }
            Wake::Retired => {
                // Retirement unlinks the socket before it writes the wake byte,
                // so every client that connected is in the drained backlog, and
                // new clients start a fresh daemon.
                accept_queued(listener, state);
                break;
            }
        }
    }
    state.wait_for_connections();
    Ok(())
}

enum Wake {
    Connection,
    Retired,
    Idle,
}

/// Blocks until a client connects, retirement wakes the loop, or
/// ``idle_timeout`` passes with neither.
fn wait_for_connection_or_retirement(
    listener: &UnixListener,
    woken: &UnixStream,
    idle_timeout: Option<Duration>,
) -> io::Result<Wake> {
    let timeout_ms = idle_timeout.map_or(-1, |timeout| {
        libc::c_int::try_from(timeout.as_millis()).unwrap_or(libc::c_int::MAX)
    });
    let mut fds = [
        libc::pollfd {
            fd: listener.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
        libc::pollfd {
            fd: woken.as_raw_fd(),
            events: libc::POLLIN,
            revents: 0,
        },
    ];
    loop {
        // SAFETY: `fds` holds two live descriptors for the call's duration.
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout_ms) };
        if ready == 0 {
            return Ok(Wake::Idle);
        }
        if ready > 0 {
            return Ok(if fds[1].revents & libc::POLLIN != 0 {
                Wake::Retired
            } else {
                Wake::Connection
            });
        }
        let err = io::Error::last_os_error();
        if err.kind() != io::ErrorKind::Interrupted {
            return Err(err);
        }
    }
}

/// Serves every connection waiting in the listener's backlog.
fn accept_queued(listener: &UnixListener, state: &Arc<DaemonState>) {
    loop {
        match listener.accept() {
            Ok((mut conn, _)) => {
                // BSD kernels hand the listener's non-blocking flag to the
                // accepted socket; connection threads block on reads.
                if let Err(err) = conn.set_nonblocking(false) {
                    eprintln!("backend daemon accept error: {err}");
                    continue;
                }
                let connection = state.connection();
                let state = Arc::clone(state);
                // Compiles run on connection threads, so they get the stack
                // compiler workers get. A refused thread drops the connection
                // (its client sees the failure) and keeps the daemon serving.
                let spawned = std::thread::Builder::new()
                    .name("molt-daemon-connection".to_string())
                    .stack_size(COMPILE_THREAD_STACK_BYTES)
                    .spawn(move || {
                        let _connection = connection;
                        if let Err(err) = handle_daemon_connection(&mut conn, &state) {
                            eprintln!("backend daemon connection error: {err}");
                        }
                    });
                if let Err(err) = spawned {
                    eprintln!("backend daemon could not start a connection thread: {err}");
                }
            }
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => return,
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => {
                // A persistent failure such as EMFILE leaves the listener
                // readable; pause instead of spinning on it.
                eprintln!("backend daemon accept error: {err}");
                std::thread::sleep(ACCEPT_ERROR_BACKOFF);
                return;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::Instant;

    static NONCE: AtomicU64 = AtomicU64::new(0);

    /// A daemon bound in its own temporary directory, removed on drop.
    struct TestDaemon {
        listener: UnixListener,
        woken: UnixStream,
        state: Arc<DaemonState>,
        socket: PathBuf,
        dir: PathBuf,
    }

    impl TestDaemon {
        fn bind() -> Self {
            // Short: a Unix socket path holds at most 104 bytes on macOS.
            let dir = std::env::temp_dir().join(format!(
                "mbd-idle-{}-{}",
                std::process::id(),
                NONCE.fetch_add(1, Ordering::Relaxed)
            ));
            std::fs::create_dir_all(&dir).expect("create the test directory");
            let socket = dir.join("daemon.sock");
            let listener = UnixListener::bind(&socket).expect("bind the daemon socket");
            listener.set_nonblocking(true).expect("nonblocking");
            let (wake, woken) = UnixStream::pair().expect("wake pair");
            let retirement = Retirement::new(socket.clone(), wake).expect("record the socket");
            let state = Arc::new(DaemonState::new(
                DaemonCache::new(Some(1024 * 1024)),
                None,
                1024 * 1024,
                1,
                1,
                retirement,
            ));
            Self {
                listener,
                woken,
                state,
                socket,
                dir,
            }
        }

        fn serve(&self, idle_timeout: Duration) -> io::Result<()> {
            serve(&self.listener, &self.woken, &self.state, Some(idle_timeout))
        }
    }

    impl Drop for TestDaemon {
        fn drop(&mut self) {
            std::fs::remove_dir_all(&self.dir).expect("clean the test directory");
        }
    }

    #[test]
    fn an_idle_daemon_retires_and_unlinks_its_socket() {
        let daemon = TestDaemon::bind();
        let started = Instant::now();
        daemon
            .serve(Duration::from_millis(200))
            .expect("the daemon serves until it retires");
        assert!(started.elapsed() >= Duration::from_millis(200));
        assert!(daemon.state.retiring());
        assert!(
            !daemon.socket.exists(),
            "a later build must start a fresh daemon"
        );
    }

    #[test]
    fn retirement_leaves_a_successor_socket_in_place() {
        let daemon = TestDaemon::bind();
        // Another daemon owns the path now, as after an external unlink.
        std::fs::remove_file(&daemon.socket).expect("unlink the first socket");
        let _successor = UnixListener::bind(&daemon.socket).expect("bind the successor");
        daemon
            .serve(Duration::from_millis(100))
            .expect("the orphaned daemon retires when idle");
        assert!(daemon.state.retiring());
        assert!(
            daemon.socket.exists(),
            "retirement removed a successor's socket"
        );
    }

    #[test]
    fn a_live_connection_keeps_an_idle_daemon_serving() {
        let timeout = Duration::from_millis(200);
        let daemon = TestDaemon::bind();
        // A client that connected and has not finished holds the daemon.
        let client = UnixStream::connect(&daemon.socket).expect("connect");
        let released = std::thread::scope(|scope| {
            let serving = scope.spawn(|| daemon.serve(timeout));
            std::thread::sleep(timeout * 5 / 2);
            assert!(
                !daemon.state.retiring(),
                "a live connection must not be retired"
            );
            let released = Instant::now();
            drop(client);
            serving
                .join()
                .expect("the serve thread")
                .expect("the daemon retires once the client leaves");
            released
        });
        // The idle period starts when the last connection ends.
        assert!(released.elapsed() >= timeout);
        assert!(!daemon.socket.exists());
    }
}
