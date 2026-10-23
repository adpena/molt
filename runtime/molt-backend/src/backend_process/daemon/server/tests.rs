use super::*;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

static NONCE: AtomicU64 = AtomicU64::new(0);

#[test]
fn interrupted_poll_preserves_its_original_wait_budget() {
    let mut fds = std::array::from_fn(|_| libc::pollfd {
        fd: -1,
        events: 0,
        revents: 0,
    });
    let mut elapsed = [
        Duration::ZERO,
        Duration::from_millis(40),
        Duration::from_millis(120),
    ]
    .into_iter();
    let mut timeouts = Vec::new();
    let result = poll_for_wake(
        &mut fds,
        Some(Duration::from_millis(100)),
        || elapsed.next().expect("one clock read per interrupted wait"),
        |_, timeout| {
            timeouts.push(timeout);
            if timeouts.len() < 3 {
                Err(io::ErrorKind::Interrupted.into())
            } else {
                Ok(false)
            }
        },
    );
    assert!(matches!(result, Ok(Wake::Idle)));
    assert_eq!(timeouts, [100, 60, 0]);
    assert!(elapsed.next().is_none());
}

#[test]
fn poll_wait_ceil_and_disabled_deadline_have_no_busy_loop() {
    for (duration, expected) in [
        (Duration::ZERO, 0),
        (Duration::from_nanos(1), 1),
        (Duration::from_micros(999), 1),
        (Duration::from_millis(1), 1),
        (Duration::from_micros(1001), 2),
        (Duration::MAX, libc::c_int::MAX),
    ] {
        let mut fds = std::array::from_fn(|_| libc::pollfd {
            fd: -1,
            events: 0,
            revents: 0,
        });
        let result = poll_for_wake(
            &mut fds,
            Some(duration),
            || Duration::ZERO,
            |_, timeout| {
                assert_eq!(timeout, expected);
                Ok(false)
            },
        );
        assert!(matches!(result, Ok(Wake::Idle)));
    }
    let mut fds = std::array::from_fn(|_| libc::pollfd {
        fd: -1,
        events: 0,
        revents: 0,
    });
    let result = poll_for_wake(
        &mut fds,
        None,
        || panic!("unbounded waits need no clock"),
        |_, timeout| {
            assert_eq!(timeout, -1);
            Err(io::ErrorKind::PermissionDenied.into())
        },
    );
    assert_eq!(
        result.err().unwrap().kind(),
        io::ErrorKind::PermissionDenied
    );
}

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
        // A Unix socket path holds at most 104 bytes on macOS, and a guarded
        // run's TMPDIR can be longer: like src/molt/backend_daemon_paths.py,
        // fall back to /tmp when the path would not fit.
        let leaf = format!(
            "mbd-idle-{}-{}",
            std::process::id(),
            NONCE.fetch_add(1, Ordering::Relaxed)
        );
        let mut dir = std::env::temp_dir().join(&leaf);
        if dir.join("daemon.sock").as_os_str().len() >= 104 {
            dir = PathBuf::from("/tmp").join(&leaf);
        }
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
