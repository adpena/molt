//! Keep compiler output out of the program's stderr, and keep it from
//! stopping the program.
//!
//! An AOT backend runs in its own process, and `molt run` shows its stderr
//! only when the build fails or when backend diagnostics were requested
//! (`src/molt/cli/wrapper_build.py`). The runner compiles in the program's
//! process. While it compiles, file descriptor 2 is one end of a socket pair.
//! A runner thread drains the other end into anonymous shared memory (the
//! ring). When the compile ends, the runner restores fd 2. It then discards
//! the text (the program compiled) or writes it to the restored stderr
//! (compilation failed).
//!
//! A forked keeper process shares the ring and waits for a control pipe to
//! close. The runner closes the pipe when it is done, after it records that
//! it handled the text. If the runner dies first, by any signal or exit, the
//! keeper finds no such record and writes the text nobody has shown yet, as
//! the CLI shows a crashed backend's stderr. With backend diagnostics
//! requested, the keeper also forwards the ring to the original stderr as it
//! fills (every 10 ms at most), so the one process that shows text also
//! records what it showed, and a crash shows no line twice.
//!
//! Compiler output cannot stop the program, and the capture depends on
//! nothing the caller controls:
//! - The compiler writes to a socket whose reading end the runner holds until
//!   the compile is over, and a runner thread always drains it. So no
//!   compiler write fails, with `EPIPE`, `EFBIG`, `ENOSPC` or any other
//!   error, and none raises `SIGPIPE` or `SIGXFSZ`. This matters: the
//!   compiler's `eprintln!` lines panic on a failed write.
//! - The capture uses no file system: not `TMPDIR`, not free disk space, not
//!   the file size limit.
//! - Only the keeper's forwarding and the runner's replay write to the
//!   caller's stderr. The keeper ignores `SIGPIPE` and `SIGXFSZ`, and the
//!   runner ignores them while it works (`main.rs`, `RunnerSignals`), so a
//!   failing stderr fails only those writes, and their errors are ignored.
//! - The keeper only waits and reads. The runner never writes to it, so a
//!   keeper that another process killed changes nothing for the runner.
//! - The thread copies each chunk into the ring before it takes the chunk out
//!   of the socket (`MSG_PEEK`), and records both steps. A keeper that finds
//!   the runner dead writes the ring, then what is left in the socket. So the
//!   last line before a crash (an allocation failure's message before its
//!   abort) reaches the caller.
//! - A closed fd 2 stays closed for the program. The capture still occupies
//!   it while the runner compiles, so no file the compiler opens can become
//!   fd 2 and receive compiler output. That output goes nowhere.
//! - Every descriptor the capture opens sits above fd 2 and closes on exec,
//!   so a closed fd 0 or 1 stays closed.
//! - If the capture cannot be set up (no descriptors, memory or thread), the
//!   runner says so and the compiler writes to fd 2 directly, as with backend
//!   diagnostics: the program still starts. Only then can an error on fd 2
//!   reach the compiler. If only the keeper cannot start, the runner captures
//!   without one.

use serde::Deserialize;

/// What the runner does with compiler stderr; the CLI decides.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CompilerDiagnostics {
    /// Backend diagnostics were requested: compiler stderr streams through.
    Stream,
    /// Show compiler stderr only if compilation fails.
    OnFailure,
}

#[cfg(unix)]
pub(crate) use unix::CapturedStderr;

#[cfg(not(unix))]
pub(crate) struct CapturedStderr;

#[cfg(not(unix))]
impl CapturedStderr {
    pub(crate) unsafe fn begin(_diagnostics: CompilerDiagnostics) -> Result<Self, String> {
        Err("compiler output capture needs a Unix file descriptor table".to_string())
    }

    pub(crate) fn finish(self, _replay: bool) {}
}

#[cfg(unix)]
mod unix {
    use super::CompilerDiagnostics;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

    /// The ring keeps the last this many bytes of compiler output. Progress
    /// lines for a whole program take a few kilobytes; only requested debug
    /// output comes near this, and it streams.
    pub(super) const RING_CAPACITY: usize = 16 << 20;

    /// The ledger's room at the start of the shared mapping.
    const LEDGER_BYTES: usize = 64;

    /// How much the drain thread peeks at once.
    const CHUNK_BYTES: usize = 16 * 1024;

    /// The ledger's verdict once the runner handled the text itself.
    const RUNNER_DONE: u32 = 1;

    /// How often a forwarding keeper looks for new compiler output.
    const FORWARD_INTERVAL_MS: libc::c_int = 10;

    /// The lowest descriptor the capture may hold: fds 0-2 are the program's.
    const FIRST_PRIVATE_FD: libc::c_int = 3;

    /// What the drain thread did, in shared memory. Each counter counts bytes
    /// from the start of the compile; the drain thread is the only writer.
    #[repr(C)]
    struct Ledger {
        /// Bytes copied into the ring (it keeps the last `RING_CAPACITY`).
        copied: AtomicU64,
        /// Bytes taken out of the socket, or about to be: the thread records
        /// them first, so a crash in between can repeat a chunk, never lose
        /// one.
        consumed: AtomicU64,
        /// Bytes already shown on the original stderr: forwarded while
        /// diagnostics stream, or replayed. Only one process shows text at a
        /// time: the keeper while it lives, then the runner.
        shown: AtomicU64,
        /// `RUNNER_DONE` once the runner handled the text.
        verdict: AtomicU32,
    }

    const _: () = assert!(std::mem::size_of::<Ledger>() <= LEDGER_BYTES);

    fn os_error(what: &str) -> String {
        format!("{what}: {}", std::io::Error::last_os_error())
    }

    fn interrupted() -> bool {
        std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
    }

    /// A close-on-exec copy of `fd` above the standard descriptors.
    fn above_stdio(fd: RawFd) -> Result<OwnedFd, String> {
        // SAFETY: F_DUPFD_CLOEXEC only reads `fd` and returns a new descriptor.
        let moved = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, FIRST_PRIVATE_FD) };
        if moved < 0 {
            return Err(os_error("cannot copy a compiler output descriptor"));
        }
        // SAFETY: `moved` is a new descriptor this process owns.
        Ok(unsafe { OwnedFd::from_raw_fd(moved) })
    }

    /// Two connected descriptors from `create`, moved above fd 2. When fd 0
    /// or 1 is closed, `create` would hand out that number; the program would
    /// then start with a stray descriptor there.
    fn pair(
        create: impl FnOnce(&mut [RawFd; 2]) -> libc::c_int,
        what: &str,
    ) -> Result<[OwnedFd; 2], String> {
        let mut ends = [-1; 2];
        if create(&mut ends) != 0 {
            return Err(os_error(what));
        }
        // SAFETY: `create` opened both descriptors for this process.
        let ends = ends.map(|fd| unsafe { OwnedFd::from_raw_fd(fd) });
        Ok([
            above_stdio(ends[0].as_raw_fd())?,
            above_stdio(ends[1].as_raw_fd())?,
        ])
    }

    /// The socket pair the compiler writes into: `[read end, write end]`.
    fn socket_pair() -> Result<[OwnedFd; 2], String> {
        pair(
            // SAFETY: `ends` has room for the two descriptors.
            |ends| unsafe {
                libc::socketpair(libc::AF_UNIX, libc::SOCK_STREAM, 0, ends.as_mut_ptr())
            },
            "cannot create the compiler output socket",
        )
    }

    /// A pipe: `[read end, write end]`.
    fn pipe() -> Result<[OwnedFd; 2], String> {
        pair(
            // SAFETY: `ends` has room for the two descriptors.
            |ends| unsafe { libc::pipe(ends.as_mut_ptr()) },
            "cannot create the compiler output keeper's pipe",
        )
    }

    /// `write` all of `bytes` to `fd`, retrying interrupted and short writes.
    /// Returns whether every byte was written; a failure is the caller's to
    /// ignore.
    pub(super) fn write_all(fd: RawFd, mut bytes: &[u8]) -> bool {
        while !bytes.is_empty() {
            // SAFETY: `bytes` is a live slice.
            let written = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
            if written > 0 {
                bytes = &bytes[written as usize..];
            } else if !(written < 0 && interrupted()) {
                return false;
            }
        }
        true
    }

    /// `recv` up to `buffer.len()` bytes with `flags`, retrying interruptions.
    /// Returns 0 at the end of the stream and on an error.
    fn receive(fd: RawFd, buffer: &mut [u8], flags: libc::c_int) -> usize {
        loop {
            // SAFETY: `buffer` is writable for its whole length.
            let read = unsafe { libc::recv(fd, buffer.as_mut_ptr().cast(), buffer.len(), flags) };
            if read >= 0 {
                return read as usize;
            }
            if !interrupted() {
                return 0;
            }
        }
    }

    /// Anonymous memory shared with the keeper: the ledger, then the ring.
    pub(super) struct Ring {
        base: std::ptr::NonNull<u8>,
    }

    // SAFETY: the mapping lives until `Ring` drops; the ledger is atomics, and
    // only the drain thread writes the ring while anyone else may read it.
    unsafe impl Send for Ring {}
    // SAFETY: as above.
    unsafe impl Sync for Ring {}

    impl Ring {
        const LEN: usize = LEDGER_BYTES + RING_CAPACITY;

        fn map() -> Result<Self, String> {
            // SAFETY: a fresh anonymous mapping; the kernel zero-fills it, so
            // every ledger counter starts at zero.
            let base = unsafe {
                libc::mmap(
                    std::ptr::null_mut(),
                    Self::LEN,
                    libc::PROT_READ | libc::PROT_WRITE,
                    libc::MAP_SHARED | libc::MAP_ANON,
                    -1,
                    0,
                )
            };
            if base == libc::MAP_FAILED {
                return Err(os_error("cannot map compiler output memory"));
            }
            Ok(Self {
                base: std::ptr::NonNull::new(base.cast()).expect("mmap returned null"),
            })
        }

        fn ledger(&self) -> &Ledger {
            // SAFETY: the ledger sits at the mapping's page-aligned start, and
            // the zeroed bytes are valid atomics.
            unsafe { &*self.base.as_ptr().cast::<Ledger>() }
        }

        fn data(&self) -> *mut u8 {
            // SAFETY: the ring follows the ledger inside the mapping.
            unsafe { self.base.as_ptr().add(LEDGER_BYTES) }
        }

        /// The ring's slices for the stream bytes `[from, to)`; the caller
        /// keeps `to - from` within `RING_CAPACITY`.
        fn slices(&self, from: u64, to: u64) -> [(*mut u8, usize); 2] {
            let start = (from % RING_CAPACITY as u64) as usize;
            let len = (to - from) as usize;
            let first = len.min(RING_CAPACITY - start);
            // SAFETY: `start < RING_CAPACITY`, so both slices lie in the ring.
            [
                (unsafe { self.data().add(start) }, first),
                (self.data(), len - first),
            ]
        }

        /// Copy `bytes` into the ring at stream offset `at`.
        fn store(&self, at: u64, bytes: &[u8]) {
            let end = at + bytes.len() as u64;
            let mut source = bytes.as_ptr();
            for (target, len) in self.slices(at, end) {
                // SAFETY: the target slice lies in the ring; only this thread
                // writes it, and `bytes` is a separate buffer.
                unsafe {
                    std::ptr::copy_nonoverlapping(source, target, len);
                    source = source.add(len);
                }
            }
        }

        /// Write to `fd` what the ring holds and nobody has shown, with a note
        /// when the ring already dropped some of it, and record it as shown.
        /// A forwarding keeper reads while the drain thread writes; only a
        /// keeper a whole ring (16 MiB) behind could read bytes the thread is
        /// overwriting, and those are bytes the ring drops anyway.
        pub(super) fn write_unshown(&self, fd: RawFd) {
            let copied = self.ledger().copied.load(Ordering::Acquire);
            let shown = self.ledger().shown.load(Ordering::Acquire);
            if copied == shown {
                return;
            }
            let kept_from = copied.saturating_sub(RING_CAPACITY as u64);
            let mut written = true;
            if kept_from > shown {
                let note = format!(
                    "molt-jit: {} earlier bytes of compiler output were dropped\n",
                    kept_from - shown
                );
                written = write_all(fd, note.as_bytes());
            }
            for (start, len) in self.slices(kept_from.max(shown), copied) {
                // SAFETY: the slice lies in the ring, below `copied`, which
                // the drain thread stores after it writes those bytes.
                let bytes = unsafe { std::slice::from_raw_parts(start, len) };
                written = written && write_all(fd, bytes);
            }
            // Recorded after the write: a process killed in between leaves
            // the text to be shown again, not lost. A failed write is not
            // retried; it stops the writes after it.
            self.ledger().shown.store(copied, Ordering::Release);
        }
    }

    impl Drop for Ring {
        fn drop(&mut self) {
            // SAFETY: the mapping is this value's; nothing uses it afterwards.
            unsafe { libc::munmap(self.base.as_ptr().cast(), Self::LEN) };
        }
    }

    /// The drain thread: copy what the compiler writes into the ring, and
    /// forward it to `forward` (diagnostics stream with no keeper to forward
    /// them), until every writer has closed the socket.
    fn drain(reader: OwnedFd, ring: Arc<Ring>, forward: Option<RawFd>) {
        let fd = reader.as_raw_fd();
        let ledger = ring.ledger();
        let mut chunk = vec![0_u8; CHUNK_BYTES];
        loop {
            let len = receive(fd, &mut chunk, libc::MSG_PEEK);
            if len == 0 {
                return;
            }
            let start = ledger.copied.load(Ordering::Relaxed);
            let end = start + len as u64;
            ring.store(start, &chunk[..len]);
            ledger.copied.store(end, Ordering::Release);
            if let Some(stderr) = forward {
                // A failing stderr loses this text and nothing else.
                ring.write_unshown(stderr);
            }
            ledger.consumed.store(end, Ordering::Release);
            let mut left = len;
            while left > 0 {
                let taken = receive(fd, &mut chunk[..left], 0);
                if taken == 0 {
                    return;
                }
                left -= taken;
            }
        }
    }

    /// The keeper process: wait until the runner closes the control pipe,
    /// forwarding the ring as it fills when `forward`. If the runner did not
    /// record that it handled the text, it died: write the text nobody has
    /// shown, then what is left in the socket.
    ///
    /// # Safety
    /// Runs in the child of `fork` of a single-threaded process and never
    /// returns.
    unsafe fn keep(ring: &Ring, reader: RawFd, control: RawFd, forward: bool) -> ! {
        // SAFETY: the keeper's own dispositions: a stderr that fails must not
        // end it before it has read the socket to its end.
        unsafe {
            libc::signal(libc::SIGPIPE, libc::SIG_IGN);
            libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
        }
        let stderr = libc::STDERR_FILENO;
        let mut buffer = vec![0_u8; CHUNK_BYTES];
        // The runner never writes here: the pipe becomes readable, at its
        // end, when the runner closes it or dies.
        loop {
            let mut control_ready = libc::pollfd {
                fd: control,
                events: libc::POLLIN,
                revents: 0,
            };
            let timeout = if forward { FORWARD_INTERVAL_MS } else { -1 };
            // SAFETY: one valid pollfd.
            let ready = unsafe { libc::poll(&mut control_ready, 1, timeout) };
            if forward {
                ring.write_unshown(stderr);
            }
            if ready > 0 {
                // SAFETY: `buffer` is writable for its whole length.
                let read = unsafe { libc::read(control, buffer.as_mut_ptr().cast(), buffer.len()) };
                if read == 0 || (read < 0 && !interrupted()) {
                    break;
                }
            } else if ready < 0 && !interrupted() {
                break;
            }
        }
        let done = ring.ledger().verdict.load(Ordering::Acquire) == RUNNER_DONE;
        if forward || !done {
            // The runner joined its drain thread before it closed the pipe,
            // or it died: the ring holds all it will hold.
            ring.write_unshown(stderr);
        }
        if !done {
            // The socket still holds the chunk the thread copied but had not
            // yet taken out: skip it.
            let ledger = ring.ledger();
            let mut skip =
                ledger.copied.load(Ordering::Acquire) - ledger.consumed.load(Ordering::Acquire);
            loop {
                let len = receive(reader, &mut buffer, 0);
                if len == 0 {
                    break;
                }
                let skipped = (skip.min(len as u64)) as usize;
                skip -= skipped as u64;
                let _ = write_all(stderr, &buffer[skipped..len]);
            }
        }
        // SAFETY: `_exit` ends the keeper without the runner's exit hooks.
        unsafe { libc::_exit(0) }
    }

    /// The keeper's process and the control pipe's write end.
    struct Keeper {
        pid: libc::pid_t,
        control: OwnedFd,
    }

    impl Keeper {
        /// Fork the keeper. It keeps `reader`, the ring and fd 2 (the
        /// original stderr), and forwards the ring when `forward`; it closes
        /// everything in `not_its_own`. `None` if the keeper cannot start:
        /// the runner then captures without one.
        ///
        /// # Safety
        /// The process must be single-threaded.
        unsafe fn start(
            ring: &Ring,
            reader: RawFd,
            forward: bool,
            not_its_own: &[RawFd],
        ) -> Option<Self> {
            let [control_read, control_write] = pipe().ok()?;
            // SAFETY: the caller guarantees a single-threaded process.
            let pid = unsafe { libc::fork() };
            if pid < 0 {
                return None;
            }
            if pid == 0 {
                // SAFETY: these are the child's copies. Stdin and stdout are
                // not the keeper's to hold, and every capture descriptor sits
                // above fd 2, so closing fds 0 and 1 closes none of them.
                unsafe {
                    for fd in not_its_own {
                        libc::close(*fd);
                    }
                    libc::close(control_write.as_raw_fd());
                    libc::close(libc::STDIN_FILENO);
                    libc::close(libc::STDOUT_FILENO);
                    keep(ring, reader, control_read.as_raw_fd(), forward)
                }
            }
            Some(Self {
                pid,
                control: control_write,
            })
        }

        /// Tell the keeper the runner handled the text, then reap it. A
        /// forwarding keeper first shows the rest of the ring.
        fn release(self, ring: &Ring) {
            ring.ledger().verdict.store(RUNNER_DONE, Ordering::Release);
            drop(self.control);
            let mut status = 0;
            // SAFETY: `pid` is this process's child; it exits once the pipe
            // closes. A caller that ignores SIGCHLD makes this ECHILD, which
            // also means the keeper is gone.
            while unsafe { libc::waitpid(self.pid, &mut status, 0) } < 0 && interrupted() {}
        }
    }

    /// File descriptor 2, captured until `finish`.
    pub(crate) struct CapturedStderr {
        /// The original fd 2; `None` when the runner started with it closed.
        saved: Option<OwnedFd>,
        diagnostics: CompilerDiagnostics,
        /// The socket end fd 2 points at while the runner compiles.
        writer: OwnedFd,
        drain: std::thread::JoinHandle<()>,
        ring: Arc<Ring>,
        keeper: Option<Keeper>,
    }

    impl CapturedStderr {
        /// Start capturing fd 2. An error means no capture could be set up:
        /// fd 2 is unchanged, and the compiler writes to it directly.
        ///
        /// # Safety
        /// The process must be single-threaded: it forks the keeper.
        pub(crate) unsafe fn begin(diagnostics: CompilerDiagnostics) -> Result<Self, String> {
            // SAFETY: F_GETFD only reads the descriptor's flags.
            let stderr_open = unsafe { libc::fcntl(libc::STDERR_FILENO, libc::F_GETFD) } >= 0;
            let [reader, writer] = socket_pair()?;
            let ring = Arc::new(Ring::map()?);
            let saved = if stderr_open {
                Some(above_stdio(libc::STDERR_FILENO)?)
            } else {
                None
            };
            let stream = diagnostics == CompilerDiagnostics::Stream;
            // A closed fd 2 has nowhere to show the text: no keeper.
            let keeper = match &saved {
                // SAFETY: the caller guarantees a single-threaded process.
                Some(saved) => unsafe {
                    Keeper::start(
                        &ring,
                        reader.as_raw_fd(),
                        stream,
                        &[writer.as_raw_fd(), saved.as_raw_fd()],
                    )
                },
                None => None,
            };
            // With no keeper, the drain thread forwards streamed diagnostics.
            let forward = match (&saved, &keeper) {
                (Some(saved), None) if stream => Some(saved.as_raw_fd()),
                _ => None,
            };
            let drained = Arc::clone(&ring);
            let drain = match std::thread::Builder::new()
                .name("molt-jit-compiler-output".to_string())
                .stack_size(64 * 1024)
                .spawn(move || drain(reader, drained, forward))
            {
                Ok(drain) => drain,
                Err(error) => {
                    if let Some(keeper) = keeper {
                        keeper.release(&ring);
                    }
                    return Err(format!("cannot start the compiler output thread: {error}"));
                }
            };
            let capture = Self {
                saved,
                diagnostics,
                writer,
                drain,
                ring,
                keeper,
            };
            // SAFETY: both descriptors are open; `dup2` atomically replaces fd 2.
            if unsafe { libc::dup2(capture.writer.as_raw_fd(), libc::STDERR_FILENO) } < 0 {
                let error = os_error("cannot redirect stderr");
                // Ending restores the fd 2 the runner started with.
                let _ = capture.end();
                return Err(error);
            }
            Ok(capture)
        }

        /// Restore fd 2 (or close it again), drain the socket and release the
        /// keeper. Returns the ring and whether the runner started with fd 2
        /// open. The process has no keeper child or capture thread afterwards.
        fn end(self) -> (Arc<Ring>, Option<OwnedFd>) {
            let Self {
                saved,
                diagnostics: _,
                writer,
                drain,
                ring,
                keeper,
            } = self;
            // SAFETY: `saved` is the original fd 2. Either way no compiler
            // write reaches the socket afterwards.
            unsafe {
                match &saved {
                    Some(saved) => libc::dup2(saved.as_raw_fd(), libc::STDERR_FILENO),
                    None => libc::close(libc::STDERR_FILENO),
                };
            }
            // The last writer closes: the thread drains to the end and exits.
            drop(writer);
            let _ = drain.join();
            if let Some(keeper) = keeper {
                keeper.release(&ring);
            }
            (ring, saved)
        }

        /// End the capture and, when `replay` is true, write the text the
        /// compiler wrote to the restored stderr. Streamed diagnostics that a
        /// keeper killed by another process did not forward appear here. A
        /// killed keeper changes nothing else: the runner holds the text.
        pub(crate) fn finish(self, replay: bool) {
            let stream = self.diagnostics == CompilerDiagnostics::Stream;
            let (ring, saved) = self.end();
            if (replay || stream) && saved.is_some() {
                ring.write_unshown(libc::STDERR_FILENO);
            }
        }

        #[cfg(test)]
        pub(super) fn keeper_pid(&self) -> Option<libc::pid_t> {
            self.keeper.as_ref().map(|keeper| keeper.pid)
        }

        #[cfg(test)]
        pub(super) fn shown(&self) -> u64 {
            self.ring.ledger().shown.load(Ordering::Acquire)
        }
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::unix::{RING_CAPACITY, write_all};
    use super::{CapturedStderr, CompilerDiagnostics};
    use std::io::Read;
    use std::process::{Command, ExitStatus, Stdio};
    use std::time::{Duration, Instant};

    /// Set in a scenario's process: run the scenario, not the test around it.
    const SCENARIO: &str = "JIT_CAPTURE_TEST_SCENARIO";
    /// What a scenario's process writes to stdout first, so a test whose name
    /// matched nothing cannot pass by running nothing.
    const RAN: &str = "capture scenario ran\n";
    /// A scenario that has not ended by then is stuck.
    const DEADLINE: Duration = Duration::from_secs(120);

    /// How the scenario's fd 2 starts.
    #[derive(Clone, Copy)]
    enum Stderr {
        /// A pipe the test reads.
        Read,
        /// A pipe whose reader is gone: every write fails with `EPIPE`.
        Broken,
    }

    /// The libtest name of the test function `test`.
    fn name_of<F: Fn()>(_test: F) -> &'static str {
        let path = std::any::type_name::<F>();
        path.split_once("::").map_or(path, |(_, name)| name)
    }

    /// Run `body` as the test `test` in a fresh copy of this test binary that
    /// runs only that test, and return what reached its fd 2 (from it and its
    /// keeper) and how it ended. The copy ignores `SIGPIPE` and `SIGXFSZ`, as
    /// the runner does while it works (`main.rs`, `RunnerSignals`).
    ///
    /// A scenario never runs in a fork of the test harness: the harness runs
    /// other tests on other threads, and a fork copies any lock one of them
    /// holds (Rust's thread-start lock among them), so the capture's thread
    /// could wait on it forever. In the copy, the scenario's thread is the only
    /// one at work, as the runner's main thread is.
    fn stderr_of_child(test: &str, stderr: Stderr, body: fn()) -> (String, ExitStatus) {
        #[expect(
            clippy::disallowed_methods,
            reason = "the test binary tells a scenario's process from the harness"
        )]
        if std::env::var_os(SCENARIO).is_some() {
            let _ = write_all(libc::STDOUT_FILENO, RAN.as_bytes());
            // SAFETY: this process's own dispositions.
            unsafe {
                libc::signal(libc::SIGPIPE, libc::SIG_IGN);
                libc::signal(libc::SIGXFSZ, libc::SIG_IGN);
            }
            body();
            leave(0);
        }
        let mut command = Command::new(std::env::current_exe().expect("the test binary's path"));
        command
            .args([test, "--exact", "--test-threads=1", "--nocapture"])
            .env(SCENARIO, "1")
            .stdin(Stdio::null())
            .stdout(Stdio::piped());
        match stderr {
            Stderr::Read => command.stderr(Stdio::piped()),
            Stderr::Broken => {
                let (reader, writer) = std::io::pipe().expect("pipe");
                drop(reader);
                command.stderr(writer)
            }
        };
        let mut child = command.spawn().expect("start the scenario");
        let drain = |pipe: Option<Box<dyn Read + Send>>| {
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                if let Some(mut pipe) = pipe {
                    let _ = pipe.read_to_end(&mut bytes);
                }
                String::from_utf8_lossy(&bytes).into_owned()
            })
        };
        let stdout = drain(child.stdout.take().map(|pipe| Box::new(pipe) as _));
        let stderr = drain(child.stderr.take().map(|pipe| Box::new(pipe) as _));
        let started = Instant::now();
        let status = loop {
            if let Some(status) = child.try_wait().expect("wait for the scenario") {
                break status;
            }
            if started.elapsed() > DEADLINE {
                let _ = child.kill();
                panic!("scenario {test} did not end within {DEADLINE:?}");
            }
            std::thread::sleep(Duration::from_millis(5));
        };
        let stdout = stdout.join().expect("read the scenario's stdout");
        assert!(
            stdout.contains(RAN),
            "no scenario named {test} ran: {stdout:?}"
        );
        (stderr.join().expect("read the scenario's stderr"), status)
    }

    fn begin(diagnostics: CompilerDiagnostics) -> CapturedStderr {
        // SAFETY: the scenario's thread is its process's only working thread.
        match unsafe { CapturedStderr::begin(diagnostics) } {
            Ok(capture) => capture,
            Err(_) => leave(2),
        }
    }

    /// End the scenario at once with `status`.
    fn leave(status: libc::c_int) -> ! {
        // SAFETY: leave without the test harness's exit hooks.
        unsafe { libc::_exit(status) }
    }

    /// End the scenario with `SIGKILL`, as the memory guard kills a runner,
    /// with no chance to tell the keeper anything. `kill` may return before
    /// the signal ends the process, so the thread waits for it.
    fn die() -> ! {
        // SAFETY: signals this scenario's own process.
        unsafe { libc::kill(libc::getpid(), libc::SIGKILL) };
        loop {
            // SAFETY: waits for the signal.
            unsafe { libc::pause() };
        }
    }

    /// End the scenario with `status` unless `holds`.
    fn check(holds: bool, status: libc::c_int) {
        if !holds {
            leave(status);
        }
    }

    /// Wait until `ready`, or end the scenario with `status`.
    fn wait_until(ready: impl Fn() -> bool, status: libc::c_int) {
        let started = Instant::now();
        while !ready() {
            check(started.elapsed() < Duration::from_secs(30), status);
            std::thread::yield_now();
        }
    }

    fn compile_then(verdict: Option<bool>) {
        let capture = begin(CompilerDiagnostics::OnFailure);
        check(write_all(libc::STDERR_FILENO, b"compiler: warning\n"), 3);
        match verdict {
            Some(replay) => {
                capture.finish(replay);
                let _ = write_all(libc::STDERR_FILENO, b"after\n");
            }
            // The runner dies while it compiles.
            None => die(),
        }
    }

    fn killed(status: ExitStatus) -> bool {
        use std::os::unix::process::ExitStatusExt;
        status.signal() == Some(libc::SIGKILL)
    }

    #[test]
    fn a_successful_compile_discards_compiler_output() {
        let (stderr, status) = stderr_of_child(
            name_of(a_successful_compile_discards_compiler_output),
            Stderr::Read,
            || compile_then(Some(false)),
        );
        assert_eq!(stderr, "after\n");
        assert!(status.success(), "{status}");
    }

    #[test]
    fn a_failed_compile_replays_compiler_output_first() {
        let (stderr, status) = stderr_of_child(
            name_of(a_failed_compile_replays_compiler_output_first),
            Stderr::Read,
            || compile_then(Some(true)),
        );
        assert_eq!(stderr, "compiler: warning\nafter\n");
        assert!(status.success(), "{status}");
    }

    #[test]
    fn a_runner_that_dies_while_compiling_still_shows_compiler_output() {
        let (stderr, status) = stderr_of_child(
            name_of(a_runner_that_dies_while_compiling_still_shows_compiler_output),
            Stderr::Read,
            || compile_then(None),
        );
        assert!(killed(status), "{status}");
        assert_eq!(stderr, "compiler: warning\n");
    }

    #[test]
    fn a_crash_shows_each_byte_once_after_streamed_diagnostics() {
        // Diagnostics stream: what the thread already forwarded must not
        // appear twice when the keeper writes the rest after a crash.
        let (stderr, status) = stderr_of_child(
            name_of(a_crash_shows_each_byte_once_after_streamed_diagnostics),
            Stderr::Read,
            || {
                let capture = begin(CompilerDiagnostics::Stream);
                check(write_all(libc::STDERR_FILENO, b"shown\n"), 3);
                wait_until(|| capture.shown() >= 6, 4);
                check(write_all(libc::STDERR_FILENO, b"pending\n"), 5);
                die();
            },
        );
        assert!(killed(status), "{status}");
        assert_eq!(stderr, "shown\npending\n");
    }

    #[test]
    fn streamed_diagnostics_reach_stderr_once() {
        let (stderr, status) = stderr_of_child(
            name_of(streamed_diagnostics_reach_stderr_once),
            Stderr::Read,
            || {
                let capture = begin(CompilerDiagnostics::Stream);
                check(write_all(libc::STDERR_FILENO, b"progress\n"), 3);
                // A failed compile replays only what was not shown.
                capture.finish(true);
                let _ = write_all(libc::STDERR_FILENO, b"after\n");
            },
        );
        assert!(status.success(), "{status}");
        assert_eq!(stderr, "progress\nafter\n");
    }

    #[test]
    fn a_killed_keeper_leaves_streamed_diagnostics_to_the_runner() {
        // The keeper forwards streamed diagnostics. Once another process
        // kills it, the runner shows the rest when the compile ends, and
        // nothing twice.
        let (stderr, status) = stderr_of_child(
            name_of(a_killed_keeper_leaves_streamed_diagnostics_to_the_runner),
            Stderr::Read,
            || {
                let capture = begin(CompilerDiagnostics::Stream);
                check(write_all(libc::STDERR_FILENO, b"shown\n"), 3);
                wait_until(|| capture.shown() >= 6, 4);
                kill_keeper(&capture);
                check(write_all(libc::STDERR_FILENO, b"later\n"), 5);
                capture.finish(false);
                let _ = write_all(libc::STDERR_FILENO, b"after\n");
            },
        );
        assert!(status.success(), "{status}, stderr {stderr:?}");
        assert_eq!(stderr, "shown\nlater\nafter\n");
    }

    // The caller's stderr is a pipe nobody reads any more. The compiler's
    // eprintln! panics on a failed write, so every write must succeed, and
    // the program must start.

    #[test]
    fn compiler_writes_succeed_when_a_captured_stderr_fails() {
        let (_, status) = stderr_of_child(
            name_of(compiler_writes_succeed_when_a_captured_stderr_fails),
            Stderr::Broken,
            || {
                let capture = begin(CompilerDiagnostics::OnFailure);
                check(write_all(libc::STDERR_FILENO, b"progress\n"), 3);
                capture.finish(true);
            },
        );
        assert!(status.success(), "{status}");
    }

    #[test]
    fn compiler_writes_succeed_when_a_streamed_stderr_fails() {
        let (_, status) = stderr_of_child(
            name_of(compiler_writes_succeed_when_a_streamed_stderr_fails),
            Stderr::Broken,
            || {
                let capture = begin(CompilerDiagnostics::Stream);
                for _ in 0..1000 {
                    check(write_all(libc::STDERR_FILENO, &[b'x'; 1024]), 3);
                }
                capture.finish(false);
            },
        );
        assert!(status.success(), "{status}");
    }

    #[test]
    fn a_full_ring_keeps_the_last_bytes_and_counts_the_rest() {
        let (stderr, status) = stderr_of_child(
            name_of(a_full_ring_keeps_the_last_bytes_and_counts_the_rest),
            Stderr::Read,
            || {
                let capture = begin(CompilerDiagnostics::OnFailure);
                let filler = vec![b'.'; 64 * 1024];
                for _ in 0..RING_CAPACITY / filler.len() {
                    check(write_all(libc::STDERR_FILENO, &filler), 3);
                }
                check(write_all(libc::STDERR_FILENO, b"the end\n"), 4);
                capture.finish(true);
            },
        );
        assert!(status.success(), "{status}");
        let note = "molt-jit: 8 earlier bytes of compiler output were dropped\n";
        assert!(stderr.starts_with(note), "{:?}", &stderr[..80]);
        assert!(stderr.ends_with(".the end\n"));
        assert_eq!(stderr.len(), note.len() + RING_CAPACITY);
    }

    #[test]
    fn a_closed_stderr_stays_closed_and_no_file_takes_its_number() {
        // An AOT executable runs with fd 2 closed; so must the program. While
        // the runner compiles, a file the compiler opens must not become fd 2
        // and receive compiler output.
        let (_, status) = stderr_of_child(
            name_of(a_closed_stderr_stays_closed_and_no_file_takes_its_number),
            Stderr::Read,
            || {
                // SAFETY: the scenario owns its descriptor table.
                unsafe { libc::close(libc::STDERR_FILENO) };
                let capture = begin(CompilerDiagnostics::OnFailure);
                check(capture.keeper_pid().is_none(), 3);
                // SAFETY: opens and closes a descriptor of this scenario.
                let opened = unsafe { libc::open(c"/dev/null".as_ptr(), libc::O_WRONLY) };
                check(opened > libc::STDERR_FILENO, 4);
                // SAFETY: as above.
                unsafe { libc::close(opened) };
                check(write_all(libc::STDERR_FILENO, b"compiler: warning\n"), 5);
                capture.finish(true);
                // SAFETY: F_GETFD only reads descriptor flags.
                check(
                    unsafe { libc::fcntl(libc::STDERR_FILENO, libc::F_GETFD) } < 0,
                    6,
                );
            },
        );
        assert!(status.success(), "{status}");
    }

    /// Kill only the keeper and wait until it is gone; `finish` reaps it.
    fn kill_keeper(capture: &CapturedStderr) {
        let Some(keeper) = capture.keeper_pid() else {
            leave(7)
        };
        // SAFETY: the keeper is this scenario's child; WNOWAIT leaves it for
        // `finish` to reap.
        unsafe {
            libc::kill(keeper, libc::SIGKILL);
            let mut info = std::mem::zeroed::<libc::siginfo_t>();
            libc::waitid(
                libc::P_PID,
                keeper as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            );
        }
    }

    // Another process kills only the keeper. The compiler's later writes must
    // still succeed, and the runner must still discard or replay the whole
    // text itself.

    #[test]
    fn a_keeper_killed_alone_leaves_the_runner_its_text() {
        let (stderr, status) = stderr_of_child(
            name_of(a_keeper_killed_alone_leaves_the_runner_its_text),
            Stderr::Read,
            || {
                let capture = begin(CompilerDiagnostics::OnFailure);
                check(write_all(libc::STDERR_FILENO, b"before\n"), 3);
                kill_keeper(&capture);
                check(write_all(libc::STDERR_FILENO, b"compiler: warning\n"), 4);
                capture.finish(true);
                let _ = write_all(libc::STDERR_FILENO, b"after\n");
            },
        );
        assert!(status.success(), "{status}, stderr {stderr:?}");
        assert_eq!(stderr, "before\ncompiler: warning\nafter\n");
    }

    #[test]
    fn a_keeper_killed_alone_still_lets_the_runner_discard() {
        let (stderr, status) = stderr_of_child(
            name_of(a_keeper_killed_alone_still_lets_the_runner_discard),
            Stderr::Read,
            || {
                let capture = begin(CompilerDiagnostics::OnFailure);
                kill_keeper(&capture);
                check(write_all(libc::STDERR_FILENO, b"compiler: warning\n"), 4);
                capture.finish(false);
                let _ = write_all(libc::STDERR_FILENO, b"after\n");
            },
        );
        assert!(status.success(), "{status}, stderr {stderr:?}");
        assert_eq!(stderr, "after\n");
    }

    #[test]
    fn closed_stdin_and_stdout_stay_closed_and_the_capture_still_replays() {
        // With fds 0 and 1 closed, `socketpair` and `pipe` would hand them
        // out; the keeper would close its own ends and the program would
        // start with a stray descriptor.
        let (stderr, status) = stderr_of_child(
            name_of(closed_stdin_and_stdout_stay_closed_and_the_capture_still_replays),
            Stderr::Read,
            || {
                // SAFETY: the scenario owns its descriptor table.
                unsafe {
                    libc::close(libc::STDIN_FILENO);
                    libc::close(libc::STDOUT_FILENO);
                }
                let capture = begin(CompilerDiagnostics::OnFailure);
                check(write_all(libc::STDERR_FILENO, b"compiler: warning\n"), 3);
                capture.finish(true);
                // SAFETY: F_GETFD only reads descriptor flags.
                let closed = |fd| unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0;
                check(closed(libc::STDIN_FILENO) && closed(libc::STDOUT_FILENO), 5);
                let _ = write_all(libc::STDERR_FILENO, b"after\n");
            },
        );
        assert!(status.success(), "{status}");
        assert_eq!(stderr, "compiler: warning\nafter\n");
    }
}
