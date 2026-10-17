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
                    Some(idle) if idle >= timeout => {
                        eprintln!("backend daemon retiring after {idle:.1?} without a connection");
                        state.retire();
                    }
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
    let started = idle_timeout.map(|_| std::time::Instant::now());
    poll_for_wake(
        &mut fds,
        idle_timeout,
        || started.expect("only timed waits read the clock").elapsed(),
        |fds, timeout_ms| {
            // SAFETY: `fds` holds two live descriptors for the call's duration.
            let ready =
                unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, timeout_ms) };
            if ready < 0 {
                Err(io::Error::last_os_error())
            } else {
                Ok(ready > 0)
            }
        },
    )
}

/// One relative wait keeps its original budget across interruptions. Generic
/// clock/poll operations let tests exercise that protocol without real signals
/// or sleeps; the actual listener uses the same loop and no allocation.
fn poll_for_wake(
    fds: &mut [libc::pollfd; 2],
    idle_timeout: Option<Duration>,
    mut elapsed: impl FnMut() -> Duration,
    mut poll: impl FnMut(&mut [libc::pollfd; 2], libc::c_int) -> io::Result<bool>,
) -> io::Result<Wake> {
    loop {
        let timeout_ms = idle_timeout.map_or(-1, |timeout| {
            // Round a positive fractional millisecond up: truncating it to
            // zero would busy-poll until the idle deadline.
            timeout
                .saturating_sub(elapsed())
                .as_nanos()
                .div_ceil(1_000_000)
                .min(libc::c_int::MAX as u128) as libc::c_int
        });
        match poll(fds, timeout_ms) {
            Ok(false) => return Ok(Wake::Idle),
            Ok(true) => {
                return Ok(if fds[1].revents & libc::POLLIN != 0 {
                    Wake::Retired
                } else {
                    Wake::Connection
                });
            }
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
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
mod tests;
