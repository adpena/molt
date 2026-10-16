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

use super::super::io_limits::{daemon_max_jobs, daemon_request_limit_bytes};
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
        Retirement::new(socket.to_path_buf(), wake),
    ));
    // Concurrent builds share one warm daemon: each connection is served on
    // its own thread, and compiles beyond the core count wait for a slot.
    loop {
        let retired = wait_for_connection_or_retirement(&listener, &woken)?;
        accept_queued(&listener, &state);
        if retired {
            // Retirement unlinks the socket before it writes the wake byte,
            // so every client that connected is in the drained backlog, and
            // new clients start a fresh daemon.
            break;
        }
    }
    state.wait_for_connections();
    Ok(())
}

/// Blocks until a client connects or retirement wakes the loop; returns
/// whether retirement did.
fn wait_for_connection_or_retirement(
    listener: &UnixListener,
    woken: &UnixStream,
) -> io::Result<bool> {
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
        let ready = unsafe { libc::poll(fds.as_mut_ptr(), fds.len() as libc::nfds_t, -1) };
        if ready >= 0 {
            return Ok(fds[1].revents & libc::POLLIN != 0);
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
