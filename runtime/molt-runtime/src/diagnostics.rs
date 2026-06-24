//! Out-of-band runtime diagnostics channel.
//!
//! Profiling and leak-gauge instrumentation — `molt_profile`, `molt_profile_mem`,
//! the `[MOLT_PROFILE] LEAK WARNING`, the `MOLT_PROFILE_JSON` payload, and the
//! `[MOLT_ASSERT_NO_LEAK]` leak report — is NOT part of a program's observable
//! behavior. It must never interleave with the program's own stdout/stderr,
//! because the differential parity harness compares those streams byte-for-byte
//! (exception-signature tests in `tests/molt_diff.py` compare stderr).
//!
//! Historically these lines were written straight to stderr via `eprintln!`, so
//! enabling the `MOLT_ASSERT_NO_LEAK` memory-safety profile — which force-enables
//! the profile counters and therefore the exit-time profile dump — silently
//! corrupted the stderr that parity tests assert on. Any stderr-comparing
//! differential test then false-failed under the leak-assert profile, so the
//! memory-corruption verification profile and the differential parity gate could
//! not be combined.
//!
//! This module routes every such diagnostic through a single sink resolved once
//! from the environment:
//!   * `MOLT_DIAGNOSTICS_FILE=<path>` — append diagnostics to that file. The
//!     differential harness points this at a per-run artifact file so the stderr
//!     it compares stays clean under any profile.
//!   * unset — stderr (back-compat: the profiler scrapes `molt_profile` from a
//!     captured stderr log under `MOLT_PROFILE`, and the `safe_run.py`
//!     leak-assert workflow surfaces the leak report on stderr for the
//!     developer; neither sets `MOLT_DIAGNOSTICS_FILE`).

use std::sync::OnceLock;

/// Environment variable naming a file that receives runtime diagnostics instead
/// of stderr. See the module docs for the rationale and the consumers.
pub(crate) const DIAGNOSTICS_FILE_ENV: &str = "MOLT_DIAGNOSTICS_FILE";

/// Where out-of-band runtime diagnostics are written.
enum DiagnosticsTarget {
    /// Default: the process stderr. Preserves the established `MOLT_PROFILE`
    /// contract (the profiler scrapes `molt_profile` from a captured stderr log)
    /// and the `safe_run.py` leak-assert workflow (leak report visible on
    /// stderr).
    Stderr,
    /// A dedicated file opened from `MOLT_DIAGNOSTICS_FILE`. Lines are appended
    /// and flushed immediately so they survive the `libc::_exit` at process
    /// teardown (which runs no buffered-writer destructors).
    File(std::sync::Mutex<std::fs::File>),
}

impl DiagnosticsTarget {
    fn write_line(&self, line: &str) {
        match self {
            DiagnosticsTarget::Stderr => eprintln!("{line}"),
            DiagnosticsTarget::File(handle) => {
                use std::io::Write;
                // A diagnostics-write failure must never perturb the program:
                // best-effort write + flush, errors ignored. A poisoned lock
                // degrades silently rather than aborting at process exit.
                if let Ok(mut file) = handle.lock() {
                    let _ = file.write_all(line.as_bytes());
                    let _ = file.write_all(b"\n");
                    let _ = file.flush();
                }
            }
        }
    }
}

/// Open `path` for append, creating it if needed. Returns `None` (so the caller
/// falls back to stderr) if the file cannot be opened.
fn open_file_target(path: &std::path::Path) -> Option<DiagnosticsTarget> {
    std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .ok()
        .map(|file| DiagnosticsTarget::File(std::sync::Mutex::new(file)))
}

fn resolve_target() -> DiagnosticsTarget {
    if let Some(raw) = std::env::var_os(DIAGNOSTICS_FILE_ENV)
        && !raw.is_empty()
        && let Some(target) = open_file_target(std::path::Path::new(&raw))
    {
        return target;
    }
    DiagnosticsTarget::Stderr
}

fn target() -> &'static DiagnosticsTarget {
    static TARGET: OnceLock<DiagnosticsTarget> = OnceLock::new();
    TARGET.get_or_init(resolve_target)
}

/// Emit a single out-of-band runtime diagnostic line.
///
/// Profiling and leak-gauge instrumentation routes through here instead of
/// writing to stderr directly, so it can never interleave with the program's own
/// stdout/stderr that the differential parity harness compares. The destination
/// is resolved once per process from [`DIAGNOSTICS_FILE_ENV`]; see the module
/// docs.
pub(crate) fn emit_line(line: &str) {
    target().write_line(line);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_target_appends_each_line_terminated() {
        let path = std::env::temp_dir().join(format!("molt_diag_unit_{}.log", std::process::id()));
        let _ = std::fs::remove_file(&path);

        let target = open_file_target(&path).expect("diagnostics file opens");
        target.write_line("molt_profile call_dispatch=0 alloc_count=1");
        target.write_line("[MOLT_ASSERT_NO_LEAK] FAIL: live_objects=5 exceeds expected_live=1");

        let contents = std::fs::read_to_string(&path).expect("diagnostics file readable");
        assert!(contents.contains("molt_profile call_dispatch=0 alloc_count=1"));
        assert!(contents.contains("[MOLT_ASSERT_NO_LEAK] FAIL: live_objects=5"));
        // Each emit is exactly one newline-terminated line, so a downstream
        // line-oriented parser sees clean records.
        assert_eq!(contents.lines().count(), 2);
        assert!(contents.ends_with('\n'));

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn unopenable_path_falls_back_to_none() {
        // Parent directory does not exist and we do not create it, so the open
        // fails and `resolve_target` falls back to stderr instead of aborting.
        let path = std::env::temp_dir()
            .join("molt_diag_nonexistent_dir_zzz")
            .join("inner.log");
        assert!(open_file_target(&path).is_none());
    }
}
