//! Keep compiler output out of the program's stderr.
//!
//! An AOT backend runs in its own process, and `molt run` shows its stderr
//! only when the build fails or when backend diagnostics were requested
//! (`src/molt/cli/wrapper_build.py`). The runner compiles in the program's
//! process, so while it compiles it points file descriptor 2 at a pipe that a
//! forked keeper process drains into memory. The runner then tells the keeper
//! to discard the text (the program compiled) or to write it to the original
//! stderr (compilation failed). If the runner dies first, by any signal or
//! exit, the keeper sees its pipes close without a verdict and writes the text,
//! as the CLI shows a crashed backend's stderr.

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

/// The keeper's verdicts. A closed control pipe with no verdict means the
/// runner died, which the keeper treats as [`REPLAY`].
const DISCARD: u8 = b'd';
const REPLAY: u8 = b'r';

/// File descriptor 2, captured by a keeper process until `finish`.
pub(crate) struct CapturedStderr {
    #[cfg(unix)]
    saved: libc::c_int,
    #[cfg(unix)]
    control: libc::c_int,
    #[cfg(unix)]
    keeper: libc::pid_t,
}

#[cfg(unix)]
fn os_error(what: &str) -> String {
    format!("{what}: {}", std::io::Error::last_os_error())
}

#[cfg(unix)]
fn pipe() -> Result<[libc::c_int; 2], String> {
    let mut ends = [-1; 2];
    // SAFETY: `ends` has room for the two descriptors `pipe` writes.
    if unsafe { libc::pipe(ends.as_mut_ptr()) } != 0 {
        return Err(os_error("cannot create a compiler output pipe"));
    }
    Ok(ends)
}

/// `write` all of `bytes` to `fd`, retrying interrupted and short writes.
#[cfg(unix)]
fn write_all(fd: libc::c_int, mut bytes: &[u8]) {
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
            return;
        }
    }
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

/// The keeper: hold the captured text, then discard or write it.
///
/// # Safety
/// Runs in the child of `fork` in a single-threaded process and never
/// returns.
#[cfg(unix)]
unsafe fn keep(capture: libc::c_int, control: libc::c_int) -> ! {
    let mut captured = Vec::new();
    read_to_end(capture, &mut captured);
    let mut verdict = Vec::new();
    read_to_end(control, &mut verdict);
    if verdict.first() != Some(&DISCARD) {
        write_all(libc::STDERR_FILENO, &captured);
    }
    // SAFETY: `_exit` ends the keeper without running the runner's exit hooks.
    unsafe { libc::_exit(0) }
}

impl CapturedStderr {
    /// Start capturing fd 2.
    ///
    /// # Safety
    /// The process must be single-threaded: it forks the keeper.
    #[cfg(unix)]
    pub(crate) unsafe fn begin() -> Result<Self, String> {
        use std::io::Write;
        let _ = std::io::stderr().flush();
        let [capture_read, capture_write] = pipe()?;
        let [control_read, control_write] = pipe()?;
        // SAFETY: the caller guarantees a single-threaded process.
        let keeper = unsafe { libc::fork() };
        if keeper < 0 {
            return Err(os_error("cannot start the compiler output keeper"));
        }
        if keeper == 0 {
            // SAFETY: these are this child's copies; fd 2 stays the original
            // stderr, and stdin and stdout are not the keeper's to hold.
            unsafe {
                libc::close(capture_write);
                libc::close(control_write);
                libc::close(libc::STDIN_FILENO);
                libc::close(libc::STDOUT_FILENO);
                keep(capture_read, control_read)
            }
        }
        // SAFETY: the read ends belong to the keeper now.
        unsafe {
            libc::close(capture_read);
            libc::close(control_read);
        }
        // SAFETY: fd 2 is open for the life of the process; `dup` copies it.
        let saved = unsafe { libc::dup(libc::STDERR_FILENO) };
        // SAFETY: both descriptors are open; `dup2` atomically replaces fd 2.
        if saved < 0 || unsafe { libc::dup2(capture_write, libc::STDERR_FILENO) } < 0 {
            let error = os_error("cannot redirect stderr");
            // SAFETY: closing every descriptor this call opened releases the
            // keeper, which then writes nothing and exits.
            unsafe {
                if saved >= 0 {
                    libc::close(saved);
                }
                libc::close(capture_write);
                libc::write(control_write, [DISCARD].as_ptr().cast(), 1);
                libc::close(control_write);
                libc::waitpid(keeper, std::ptr::null_mut(), 0);
            }
            return Err(error);
        }
        // SAFETY: fd 2 now holds the write end.
        unsafe { libc::close(capture_write) };
        Ok(Self {
            saved,
            control: control_write,
            keeper,
        })
    }

    #[cfg(not(unix))]
    pub(crate) unsafe fn begin() -> Result<Self, String> {
        Err("compiler output capture needs a Unix file descriptor table".to_string())
    }

    /// Restore fd 2 and wait for the keeper: it writes what the compiler
    /// wrote when `replay` is true and discards it otherwise. The process has
    /// no keeper child afterwards.
    pub(crate) fn finish(self, replay: bool) {
        #[cfg(unix)]
        {
            use std::io::Write;
            let Self {
                saved,
                control,
                keeper,
            } = self;
            let _ = std::io::stderr().flush();
            write_all(control, &[if replay { REPLAY } else { DISCARD }]);
            // SAFETY: `saved` is the original fd 2; restoring it closes the
            // last write end of the capture pipe, so the keeper reads to its
            // end, then reads the verdict and exits.
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
    /// wait status.
    fn stderr_of_child(body: fn()) -> (String, libc::c_int) {
        let [read, write] = pipe().expect("pipe");
        // SAFETY: the child only redirects fd 2, runs `body` and exits.
        let child = unsafe { libc::fork() };
        assert!(child >= 0, "fork");
        if child == 0 {
            // SAFETY: the child owns its descriptor table.
            unsafe {
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

    fn compile_then(verdict: Option<bool>) {
        // SAFETY: the forked child is single-threaded.
        let capture = unsafe { CapturedStderr::begin() }.expect("begin capture");
        write_all(libc::STDERR_FILENO, b"compiler: warning\n");
        match verdict {
            Some(replay) => {
                capture.finish(replay);
                write_all(libc::STDERR_FILENO, b"after\n");
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
}
