//! Keep compiler output out of the program's stderr.
//!
//! An AOT backend runs in its own process, and `molt run` shows its stderr
//! only when the build fails or when backend diagnostics were requested
//! (`src/molt/cli/wrapper_build.py`). The runner compiles in the program's
//! process, so while it compiles it points file descriptor 2 at an unlinked
//! capture file. Both the runner and a forked keeper process hold that file.
//! When the compile ends, the runner discards the text (the program compiled)
//! or writes it to the original stderr itself (compilation failed), and tells
//! the keeper to exit. If the runner dies first, by any signal or exit, the
//! keeper sees the control pipe close without a verdict and writes the text,
//! as the CLI shows a crashed backend's stderr.
//!
//! The capture must not change how the program starts, whatever state the
//! runner inherited or whatever happens to the keeper:
//! - A closed fd 2 has nothing to capture, so the runner compiles with it
//!   closed, as an AOT executable runs with it closed.
//! - Every descriptor the capture opens sits above fd 2 and closes on exec,
//!   so a closed fd 0 or 1 stays closed.
//! - Compiler writes go to a regular file, never to a pipe, so they cannot
//!   fail with `EPIPE`: the compiler's `eprintln!` progress lines panic on a
//!   write error, and a pipe whose keeper another process killed would turn
//!   every later line into one. The keeper is needed only if the runner dies;
//!   if the keeper dies instead, the runner still holds the text, and the
//!   program starts as it would have. Only the one-byte verdict goes through
//!   a pipe, and a dead keeper makes that write fail with `EPIPE` instead of
//!   raising `SIGPIPE` (macOS `F_SETNOSIGPIPE`; on Linux the runner blocks
//!   `SIGPIPE` while it captures and discards what is pending).

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

/// The verdict a live runner sends: it handles the text itself, so the keeper
/// exits without writing. A control pipe that closes with no verdict means the
/// runner died, and the keeper writes the text.
const RUNNER_ALIVE: u8 = b'd';

/// File descriptor 2, captured into a file until `finish`.
pub(crate) struct CapturedStderr {
    #[cfg(unix)]
    saved: libc::c_int,
    #[cfg(unix)]
    capture: libc::c_int,
    #[cfg(unix)]
    control: libc::c_int,
    #[cfg(unix)]
    keeper: libc::pid_t,
    #[cfg(all(unix, not(target_os = "macos")))]
    sigpipe: SigpipeBlocked,
}

#[cfg(unix)]
fn os_error(what: &str) -> String {
    format!("{what}: {}", std::io::Error::last_os_error())
}

/// The lowest descriptor the capture may hold: fds 0-2 are the program's.
#[cfg(unix)]
const FIRST_PRIVATE_FD: libc::c_int = 3;

/// A close-on-exec copy of `fd` above the standard descriptors; closes `fd`.
#[cfg(unix)]
fn above_stdio(fd: libc::c_int) -> Result<libc::c_int, String> {
    // SAFETY: `fd` is open; F_DUPFD_CLOEXEC returns a new descriptor.
    let moved = unsafe { libc::fcntl(fd, libc::F_DUPFD_CLOEXEC, FIRST_PRIVATE_FD) };
    let error = (moved < 0).then(|| os_error("cannot move a compiler output descriptor"));
    // SAFETY: `fd` is this call's to close; `moved` now holds the same file.
    unsafe { libc::close(fd) };
    error.map_or(Ok(moved), Err)
}

/// Close every descriptor in `fds`.
#[cfg(unix)]
fn close_all(fds: &[libc::c_int]) {
    for fd in fds {
        // SAFETY: each descriptor is the caller's to close.
        unsafe { libc::close(*fd) };
    }
}

/// A pipe whose ends sit above fd 2 and close on exec. When fd 0 or 1 is
/// closed, `pipe` would hand out that number; the keeper would then close its
/// own pipe end, and the program would start with a stray descriptor there.
#[cfg(unix)]
fn pipe() -> Result<[libc::c_int; 2], String> {
    let mut ends = [-1; 2];
    // SAFETY: `ends` has room for the two descriptors `pipe` writes.
    if unsafe { libc::pipe(ends.as_mut_ptr()) } != 0 {
        return Err(os_error("cannot create a compiler output pipe"));
    }
    let read = above_stdio(ends[0]);
    let write = above_stdio(ends[1]);
    match (read, write) {
        (Ok(read), Ok(write)) => Ok([read, write]),
        (read, write) => {
            for end in [read.as_ref().ok(), write.as_ref().ok()]
                .into_iter()
                .flatten()
            {
                close_all(&[*end]);
            }
            Err(read.err().or(write.err()).unwrap_or_default())
        }
    }
}

/// An unlinked, close-on-exec file above fd 2 in the temporary directory, to
/// hold what the compiler writes. Only its open descriptors keep it alive.
#[cfg(unix)]
fn capture_file() -> Result<libc::c_int, String> {
    use std::os::unix::ffi::OsStrExt;
    let template = std::env::temp_dir().join("molt-jit-compiler-output.XXXXXX");
    let mut path = template.as_os_str().as_bytes().to_vec();
    path.push(0);
    // SAFETY: `path` is a writable NUL-terminated template; mkstemp fills in
    // its X's and opens the file it names.
    let fd = unsafe { libc::mkstemp(path.as_mut_ptr().cast()) };
    if fd < 0 {
        return Err(os_error(&format!(
            "cannot create a compiler output file in {}",
            std::env::temp_dir().display()
        )));
    }
    // SAFETY: `path` now names the file `fd` opened; nothing else uses it.
    unsafe { libc::unlink(path.as_ptr().cast()) };
    above_stdio(fd)
}

/// Writes to `fd` fail with `EPIPE` instead of raising `SIGPIPE` once its
/// reader is gone.
#[cfg(target_os = "macos")]
fn without_sigpipe(fd: libc::c_int) -> Result<(), String> {
    // SAFETY: `fd` is open; F_SETNOSIGPIPE takes an int flag.
    if unsafe { libc::fcntl(fd, libc::F_SETNOSIGPIPE, 1) } < 0 {
        return Err(os_error(
            "cannot keep SIGPIPE off the compiler output keeper",
        ));
    }
    Ok(())
}

#[cfg(all(unix, not(target_os = "macos")))]
fn without_sigpipe(_fd: libc::c_int) -> Result<(), String> {
    Ok(())
}

/// Linux sends `SIGPIPE` to the writing thread. While the capture holds fd 2,
/// the runner's threads (and the compiler threads they start, which inherit
/// the mask) keep it blocked; `restore` discards what is pending and restores
/// the start-up mask, so the program starts with the mask it inherited.
#[cfg(all(unix, not(target_os = "macos")))]
struct SigpipeBlocked(libc::sigset_t);

#[cfg(all(unix, not(target_os = "macos")))]
impl SigpipeBlocked {
    fn block() -> Self {
        // SAFETY: plain signal-set manipulation on owned values.
        unsafe {
            let mut pipe_only = std::mem::zeroed::<libc::sigset_t>();
            libc::sigemptyset(&mut pipe_only);
            libc::sigaddset(&mut pipe_only, libc::SIGPIPE);
            let mut previous = std::mem::zeroed::<libc::sigset_t>();
            libc::pthread_sigmask(libc::SIG_BLOCK, &pipe_only, &mut previous);
            Self(previous)
        }
    }

    fn restore(self) {
        // SAFETY: plain signal-set manipulation; a zero timeout never waits.
        unsafe {
            let mut pipe_only = std::mem::zeroed::<libc::sigset_t>();
            libc::sigemptyset(&mut pipe_only);
            libc::sigaddset(&mut pipe_only, libc::SIGPIPE);
            let now = libc::timespec {
                tv_sec: 0,
                tv_nsec: 0,
            };
            while libc::sigtimedwait(&pipe_only, std::ptr::null_mut(), &now) == libc::SIGPIPE {}
            libc::pthread_sigmask(libc::SIG_SETMASK, &self.0, std::ptr::null_mut());
        }
    }
}

/// `write` all of `bytes` to `fd`, retrying interrupted and short writes.
/// Returns whether every byte was written.
#[cfg(unix)]
fn write_all(fd: libc::c_int, mut bytes: &[u8]) -> bool {
    while !bytes.is_empty() {
        // SAFETY: `bytes` is a live slice; `fd` is open.
        let written = unsafe { libc::write(fd, bytes.as_ptr().cast(), bytes.len()) };
        if written > 0 {
            bytes = &bytes[written as usize..];
        } else if written < 0
            && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
        {
            continue;
        } else {
            return false;
        }
    }
    true
}

/// Read `fd` to its end, retrying interrupted reads.
#[cfg(unix)]
fn read_to_end(fd: libc::c_int, output: &mut Vec<u8>) {
    let mut buffer = [0_u8; 16 * 1024];
    loop {
        // SAFETY: `buffer` is writable for its whole length; `fd` is open.
        let read = unsafe { libc::read(fd, buffer.as_mut_ptr().cast(), buffer.len()) };
        if read > 0 {
            output.extend_from_slice(&buffer[..read as usize]);
        } else if read < 0
            && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
        {
            continue;
        } else {
            return;
        }
    }
}

/// Write the whole capture file, from its start, to `fd`.
#[cfg(unix)]
fn replay(capture: libc::c_int, fd: libc::c_int) {
    let mut buffer = [0_u8; 16 * 1024];
    let mut offset: libc::off_t = 0;
    loop {
        // SAFETY: `buffer` is writable for its whole length; `capture` is
        // open. `pread` leaves the shared file offset alone.
        let read =
            unsafe { libc::pread(capture, buffer.as_mut_ptr().cast(), buffer.len(), offset) };
        if read > 0 {
            if !write_all(fd, &buffer[..read as usize]) {
                return;
            }
            offset += read as libc::off_t;
        } else if read < 0
            && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
        {
            continue;
        } else {
            return;
        }
    }
}

/// The keeper: wait for the runner's verdict; if none comes, the runner died,
/// so write what the compiler wrote.
///
/// # Safety
/// Runs in the child of `fork` in a single-threaded process and never
/// returns.
#[cfg(unix)]
unsafe fn keep(capture: libc::c_int, control: libc::c_int) -> ! {
    let mut verdict = Vec::new();
    read_to_end(control, &mut verdict);
    if verdict.first() != Some(&RUNNER_ALIVE) {
        replay(capture, libc::STDERR_FILENO);
    }
    // SAFETY: `_exit` ends the keeper without running the runner's exit hooks.
    unsafe { libc::_exit(0) }
}

impl CapturedStderr {
    /// Start capturing fd 2. Returns `None` when fd 2 is closed: there is
    /// nothing to capture, and the compiler's writes fail as they would in a
    /// backend started with stderr closed.
    ///
    /// # Safety
    /// The process must be single-threaded: it forks the keeper.
    #[cfg(unix)]
    pub(crate) unsafe fn begin() -> Result<Option<Self>, String> {
        use std::io::Write;
        // SAFETY: F_GETFD only reads the descriptor's flags.
        if unsafe { libc::fcntl(libc::STDERR_FILENO, libc::F_GETFD) } < 0 {
            return Ok(None);
        }
        let _ = std::io::stderr().flush();
        let capture = capture_file()?;
        let [control_read, control_write] = pipe().inspect_err(|_| close_all(&[capture]))?;
        if let Err(error) = without_sigpipe(control_write) {
            close_all(&[capture, control_read, control_write]);
            return Err(error);
        }
        #[cfg(not(target_os = "macos"))]
        let sigpipe = SigpipeBlocked::block();
        // SAFETY: the caller guarantees a single-threaded process.
        let keeper = unsafe { libc::fork() };
        if keeper < 0 {
            let error = os_error("cannot start the compiler output keeper");
            close_all(&[capture, control_read, control_write]);
            #[cfg(not(target_os = "macos"))]
            sigpipe.restore();
            return Err(error);
        }
        if keeper == 0 {
            // SAFETY: these are this child's copies; fd 2 stays the original
            // stderr, and stdin and stdout are not the keeper's to hold. Every
            // descriptor of the capture sits above fd 2, so closing fds 0 and
            // 1 closes none of them.
            unsafe {
                libc::close(control_write);
                libc::close(libc::STDIN_FILENO);
                libc::close(libc::STDOUT_FILENO);
                keep(capture, control_read)
            }
        }
        // The control read end belongs to the keeper now.
        close_all(&[control_read]);
        // SAFETY: fd 2 is open (checked above); the copy sits above fd 2.
        let saved =
            unsafe { libc::fcntl(libc::STDERR_FILENO, libc::F_DUPFD_CLOEXEC, FIRST_PRIVATE_FD) };
        // SAFETY: both descriptors are open; `dup2` atomically replaces fd 2.
        if saved < 0 || unsafe { libc::dup2(capture, libc::STDERR_FILENO) } < 0 {
            let error = os_error("cannot redirect stderr");
            if saved >= 0 {
                close_all(&[saved]);
            }
            // The verdict releases the keeper, which then writes nothing.
            let _ = write_all(control_write, &[RUNNER_ALIVE]);
            close_all(&[capture, control_write]);
            // SAFETY: `keeper` is this process's child.
            unsafe { libc::waitpid(keeper, std::ptr::null_mut(), 0) };
            #[cfg(not(target_os = "macos"))]
            sigpipe.restore();
            return Err(error);
        }
        Ok(Some(Self {
            saved,
            capture,
            control: control_write,
            keeper,
            #[cfg(not(target_os = "macos"))]
            sigpipe,
        }))
    }

    #[cfg(not(unix))]
    pub(crate) unsafe fn begin() -> Result<Option<Self>, String> {
        Err("compiler output capture needs a Unix file descriptor table".to_string())
    }

    /// Restore fd 2, release the keeper and, when `replay` is true, write
    /// what the compiler wrote to the restored stderr. The process has no
    /// keeper child afterwards. A keeper that another process killed changes
    /// nothing here: the runner holds the text itself.
    pub(crate) fn finish(self, replay: bool) {
        #[cfg(unix)]
        {
            use std::io::Write;
            let Self {
                saved,
                capture,
                control,
                keeper,
                #[cfg(not(target_os = "macos"))]
                sigpipe,
            } = self;
            let _ = std::io::stderr().flush();
            // A dead keeper makes this fail with EPIPE; nothing depends on it.
            let _ = write_all(control, &[RUNNER_ALIVE]);
            // SAFETY: `saved` is the original fd 2; `keeper` is this
            // process's child, which exits once it reads the verdict or EOF.
            unsafe {
                libc::close(control);
                libc::dup2(saved, libc::STDERR_FILENO);
                libc::close(saved);
                let mut status = 0;
                while libc::waitpid(keeper, &mut status, 0) < 0
                    && std::io::Error::last_os_error().kind() == std::io::ErrorKind::Interrupted
                {
                }
            }
            if replay {
                self::replay(capture, libc::STDERR_FILENO);
            }
            close_all(&[capture]);
            #[cfg(not(target_os = "macos"))]
            sigpipe.restore();
        }
        #[cfg(not(unix))]
        let _ = replay;
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    /// Run `body` in a forked child whose fd 2 is a fresh pipe; return what
    /// reached that pipe (from the child and its keeper) and the child's
    /// wait status. The child has the runner's `SIGPIPE` disposition, the
    /// default: the test harness's Rust `main` ignores the signal.
    fn stderr_of_child(body: fn()) -> (String, libc::c_int) {
        let [read, write] = pipe().expect("pipe");
        // SAFETY: the child only redirects fd 2, runs `body` and exits.
        let child = unsafe { libc::fork() };
        assert!(child >= 0, "fork");
        if child == 0 {
            // SAFETY: the child owns its descriptor table and dispositions.
            unsafe {
                libc::signal(libc::SIGPIPE, libc::SIG_DFL);
                libc::close(read);
                libc::dup2(write, libc::STDERR_FILENO);
                libc::close(write);
            }
            body();
            // SAFETY: leave without the test harness's exit hooks.
            unsafe { libc::_exit(0) }
        }
        // SAFETY: the parent keeps only the read end.
        unsafe { libc::close(write) };
        let mut output = Vec::new();
        read_to_end(read, &mut output);
        let mut status = 0;
        // SAFETY: `child` is this process's child.
        unsafe {
            libc::close(read);
            libc::waitpid(child, &mut status, 0);
        }
        (String::from_utf8_lossy(&output).into_owned(), status)
    }

    fn begin() -> CapturedStderr {
        // SAFETY: the forked child is single-threaded.
        unsafe { CapturedStderr::begin() }
            .expect("begin capture")
            .expect("fd 2 is open")
    }

    fn compile_then(verdict: Option<bool>) {
        let capture = begin();
        let _ = write_all(libc::STDERR_FILENO, b"compiler: warning\n");
        match verdict {
            Some(replay) => {
                capture.finish(replay);
                let _ = write_all(libc::STDERR_FILENO, b"after\n");
            }
            // The runner dies while it compiles: killed, as by the memory
            // guard, with no chance to give the keeper a verdict.
            // SAFETY: ends the child at once.
            None => unsafe {
                libc::kill(libc::getpid(), libc::SIGKILL);
            },
        }
    }

    #[test]
    fn a_successful_compile_discards_compiler_output() {
        let (stderr, status) = stderr_of_child(|| compile_then(Some(false)));
        assert_eq!(stderr, "after\n");
        assert!(libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0);
    }

    #[test]
    fn a_failed_compile_replays_compiler_output_first() {
        let (stderr, _) = stderr_of_child(|| compile_then(Some(true)));
        assert_eq!(stderr, "compiler: warning\nafter\n");
    }

    #[test]
    fn a_runner_that_dies_while_compiling_still_shows_compiler_output() {
        let (stderr, status) = stderr_of_child(|| compile_then(None));
        assert!(libc::WIFSIGNALED(status), "status {status:#x}");
        assert_eq!(stderr, "compiler: warning\n");
    }

    /// End the child with `status` unless `holds`.
    fn require(holds: bool, status: libc::c_int) {
        if !holds {
            // SAFETY: leave without the test harness's exit hooks.
            unsafe { libc::_exit(status) }
        }
    }

    #[test]
    fn a_runner_started_with_stderr_closed_compiles_without_a_capture() {
        // An AOT executable runs with fd 2 closed; so must the runner.
        let (_, status) = stderr_of_child(|| {
            // SAFETY: the child owns its descriptor table.
            unsafe { libc::close(libc::STDERR_FILENO) };
            // SAFETY: the forked child is single-threaded.
            let capture = unsafe { CapturedStderr::begin() };
            require(matches!(capture, Ok(None)), 3);
        });
        assert!(
            libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
            "status {status:#x}"
        );
    }

    /// Kill only the keeper and wait until it is gone; `finish` reaps it.
    fn kill_keeper(capture: &CapturedStderr) {
        // SAFETY: the keeper is this child's child; WNOWAIT leaves it for
        // `finish` to reap.
        unsafe {
            libc::kill(capture.keeper, libc::SIGKILL);
            let mut info = std::mem::zeroed::<libc::siginfo_t>();
            libc::waitid(
                libc::P_PID,
                capture.keeper as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOWAIT,
            );
        }
    }

    #[test]
    fn a_keeper_killed_alone_changes_nothing_for_the_runner() {
        // Another process kills only the keeper. The compiler's later writes
        // must still succeed (its eprintln! would panic on EPIPE), and the
        // runner must still discard or replay the whole text itself.
        for replay in [false, true] {
            let body: fn() = if replay {
                || {
                    let capture = begin();
                    let _ = write_all(libc::STDERR_FILENO, b"before\n");
                    kill_keeper(&capture);
                    require(write_all(libc::STDERR_FILENO, b"compiler: warning\n"), 4);
                    capture.finish(true);
                    let _ = write_all(libc::STDERR_FILENO, b"after\n");
                }
            } else {
                || {
                    let capture = begin();
                    kill_keeper(&capture);
                    require(write_all(libc::STDERR_FILENO, b"compiler: warning\n"), 4);
                    capture.finish(false);
                    let _ = write_all(libc::STDERR_FILENO, b"after\n");
                }
            };
            let (stderr, status) = stderr_of_child(body);
            assert!(
                libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
                "status {status:#x}, stderr {stderr:?}"
            );
            let expected = if replay {
                "before\ncompiler: warning\nafter\n"
            } else {
                "after\n"
            };
            assert_eq!(stderr, expected);
        }
    }

    #[test]
    fn closed_stdin_and_stdout_stay_closed_and_the_capture_still_replays() {
        // With fds 0 and 1 closed, `pipe` would hand them out; the keeper
        // would close its own pipe end and the program would start with a
        // stray descriptor.
        let (stderr, status) = stderr_of_child(|| {
            // SAFETY: the child owns its descriptor table.
            unsafe {
                libc::close(libc::STDIN_FILENO);
                libc::close(libc::STDOUT_FILENO);
            }
            let capture = begin();
            let _ = write_all(libc::STDERR_FILENO, b"compiler: warning\n");
            capture.finish(true);
            // SAFETY: F_GETFD only reads descriptor flags.
            let closed = |fd| unsafe { libc::fcntl(fd, libc::F_GETFD) } < 0;
            require(closed(libc::STDIN_FILENO) && closed(libc::STDOUT_FILENO), 5);
            let _ = write_all(libc::STDERR_FILENO, b"after\n");
        });
        assert!(
            libc::WIFEXITED(status) && libc::WEXITSTATUS(status) == 0,
            "status {status:#x}"
        );
        assert_eq!(stderr, "compiler: warning\nafter\n");
    }
}
