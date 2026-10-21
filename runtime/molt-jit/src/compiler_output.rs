//! Keep compiler output out of the program's stderr.
//!
//! An AOT backend runs in its own process, and `molt run` shows its stderr
//! only when the build fails or when backend diagnostics were requested
//! (`src/molt/cli/wrapper_build.py`). The runner compiles in the program's
//! process, so it holds file descriptor 2 in a private file while it compiles
//! and replays that file only under the same two conditions.

use serde::Deserialize;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom, Write};

/// What the runner does with compiler stderr; the CLI decides.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "snake_case")]
pub(crate) enum CompilerDiagnostics {
    /// Backend diagnostics were requested: compiler stderr streams through.
    Stream,
    /// Show compiler stderr only if compilation fails.
    OnFailure,
}

/// File descriptor 2, redirected into an anonymous file until `finish`.
pub(crate) struct CapturedStderr {
    #[cfg(unix)]
    saved: libc::c_int,
    #[cfg(unix)]
    file: File,
}

impl CapturedStderr {
    #[cfg(unix)]
    pub(crate) fn begin() -> Result<Self, String> {
        use std::os::fd::AsRawFd;
        let file = anonymous_file()?;
        // SAFETY: fd 2 is open for the life of the process; `dup` copies it.
        let saved = unsafe { libc::dup(libc::STDERR_FILENO) };
        if saved < 0 {
            return Err(format!(
                "cannot save stderr: {}",
                std::io::Error::last_os_error()
            ));
        }
        let _ = std::io::stderr().flush();
        // SAFETY: both descriptors are open; `dup2` atomically replaces fd 2.
        if unsafe { libc::dup2(file.as_raw_fd(), libc::STDERR_FILENO) } < 0 {
            let error = std::io::Error::last_os_error();
            // SAFETY: `saved` is the descriptor `dup` returned above.
            unsafe { libc::close(saved) };
            return Err(format!("cannot redirect stderr: {error}"));
        }
        Ok(Self { saved, file })
    }

    #[cfg(not(unix))]
    pub(crate) fn begin() -> Result<Self, String> {
        Err("compiler output capture needs a Unix file descriptor table".to_string())
    }

    /// Restore fd 2; replay what the compiler wrote when `replay` is true.
    pub(crate) fn finish(self, replay: bool) {
        #[cfg(unix)]
        {
            let Self { saved, mut file } = self;
            let _ = std::io::stderr().flush();
            // SAFETY: `saved` is the original fd 2, still open.
            unsafe {
                libc::dup2(saved, libc::STDERR_FILENO);
                libc::close(saved);
            }
            if replay {
                let mut output = Vec::new();
                if file.seek(SeekFrom::Start(0)).is_ok() && file.read_to_end(&mut output).is_ok() {
                    let mut stderr = std::io::stderr().lock();
                    let _ = stderr.write_all(&output);
                    let _ = stderr.flush();
                }
            }
        }
        #[cfg(not(unix))]
        let _ = replay;
    }
}

/// A file with no name: created exclusively in the temporary directory and
/// unlinked at once, so nothing remains if the process dies.
#[cfg(unix)]
fn anonymous_file() -> Result<File, String> {
    let directory = std::env::temp_dir();
    for attempt in 0..16_u32 {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |elapsed| elapsed.subsec_nanos());
        let path = directory.join(format!(
            "molt-jit-compiler-stderr-{}-{nanos:09}-{attempt}",
            std::process::id()
        ));
        match std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(file) => {
                let _ = std::fs::remove_file(&path);
                return Ok(file);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => {
                return Err(format!(
                    "cannot create a compiler output file in {}: {error}",
                    directory.display()
                ));
            }
        }
    }
    Err(format!(
        "cannot create a unique compiler output file in {}",
        directory.display()
    ))
}
