//! One admission check for every child process the runtime launches (HF-519).
//!
//! A compiled program is not an interpreter: its binary runs `main` whatever
//! its arguments say. The CPython idiom `[sys.executable, "-c", source]`
//! therefore starts the program again, and a program that does so at start-up
//! recurses without end. Leading projects never let a child guess its role
//! from argument shapes; the parent declares it (Chromium `--type=<role>`,
//! moby `pkg/reexec` named entry points, CPython `--multiprocessing-fork`).
//! This module follows them:
//!
//! 1. A launch of the running binary, decided by file identity, whose
//!    arguments ask for an interpreter mode (`-c` or `-m`, after CPython's
//!    interpreter options) is refused in the parent.
//! 2. A launch of the running binary passes the child a self-spawn depth one
//!    above the parent's. At depth 1 or more, a process may launch its own
//!    binary only in a declared [`SelfSpawnRole`]. This bounds every other
//!    recursion without guessing at arguments.
//!
//! The refusal is `OSError(ENOEXEC)`, the error CPython raises for a file it
//! cannot execute, so `except OSError` handlers behave as they would there.

use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

use molt_runtime_platform::file_identity::FileIdentity;

use crate::PyToken;
use crate::builtins::exceptions::raise_os_error_errno;

/// The environment entry that carries the self-spawn depth to a child. The
/// spawn authority owns it; a value set by hand can only raise the depth.
const DEPTH_ENV: &str = "MOLT_SELF_SPAWN_DEPTH";

/// The interpreter mode a launch asks for (CPython `Python/getopt.c`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum InterpreterInvocation {
    /// `-c command`: run source text.
    Command,
    /// `-m module`: run a module as a script.
    Module,
}

impl InterpreterInvocation {
    const fn flag(self) -> &'static str {
        match self {
            Self::Command => "-c",
            Self::Module => "-m",
        }
    }
}

/// A declared reason for a process to launch its own binary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelfSpawnRole {
    /// A `multiprocessing` child: the start path sets `MOLT_MP_SPAWN=1`, and
    /// CPython's frozen command line passes `--multiprocessing-fork` first.
    Multiprocessing,
}

/// Why a launch of the running binary is refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SelfSpawnRefusal {
    InterpreterInvocation(InterpreterInvocation),
    UndeclaredRole { depth: u32 },
}

impl SelfSpawnRefusal {
    fn message(self, program: &OsStr) -> String {
        let program = program.to_string_lossy();
        match self {
            Self::InterpreterInvocation(invocation) => format!(
                "[Errno {}] Exec format error: '{program}' is a compiled program with no \
                 interpreter, so it cannot run '{}'; call the code directly, or use \
                 multiprocessing with freeze_support() for worker processes",
                libc::ENOEXEC,
                invocation.flag(),
            ),
            Self::UndeclaredRole { depth } => format!(
                "[Errno {}] Exec format error: '{program}' was started by its own binary \
                 (self-spawn depth {depth}); starting it again without a declared role \
                 would recurse; use multiprocessing for worker processes",
                libc::ENOEXEC,
            ),
        }
    }

    pub(crate) fn raise(self, py: &PyToken<'_>, program: &OsStr) -> u64 {
        raise_os_error_errno(py, i64::from(libc::ENOEXEC), &self.message(program))
    }
}

/// Admit `command` for launch: refuse a self-spawn the binary cannot serve,
/// and mark an admitted self-spawn with the child's depth.
pub(crate) fn admit_child(command: &mut Command) -> Result<(), SelfSpawnRefusal> {
    let Some(own) = running_binary() else {
        return Ok(());
    };
    if !program_candidates(command)
        .into_iter()
        .find_map(|candidate| FileIdentity::of_path(&candidate).ok())
        .is_some_and(|target| target == *own)
    {
        return Ok(());
    }
    let args: Vec<&OsStr> = command.get_args().collect();
    if let Some(invocation) = interpreter_invocation(&args) {
        return Err(SelfSpawnRefusal::InterpreterInvocation(invocation));
    }
    let depth = own_depth();
    if declared_role(command, &args).is_none() && depth >= 1 {
        return Err(SelfSpawnRefusal::UndeclaredRole { depth });
    }
    let requested = explicit_env(command, DEPTH_ENV)
        .map(parse_depth)
        .unwrap_or(0);
    let child_depth = depth.saturating_add(1).max(requested);
    command.env(DEPTH_ENV, child_depth.to_string());
    Ok(())
}

/// The running image's identity, read once. Linux names the mapped image
/// through `/proc/self/exe`, which survives a rename of the path.
fn running_binary() -> Option<&'static FileIdentity> {
    static OWN: OnceLock<Option<FileIdentity>> = OnceLock::new();
    OWN.get_or_init(|| {
        #[cfg(target_os = "linux")]
        if let Ok(identity) = FileIdentity::of_path(Path::new("/proc/self/exe")) {
            return Some(identity);
        }
        let exe = std::env::current_exe().ok()?;
        FileIdentity::of_path(&exe).ok()
    })
    .as_ref()
}

/// This process's self-spawn depth, read once. A malformed value counts as
/// depth 1: it can make refusal stricter, never looser.
fn own_depth() -> u32 {
    static DEPTH: OnceLock<u32> = OnceLock::new();
    *DEPTH.get_or_init(|| std::env::var_os(DEPTH_ENV).map_or(0, |value| parse_depth(&value)))
}

fn parse_depth(value: &OsStr) -> u32 {
    value
        .to_str()
        .and_then(|text| text.parse().ok())
        .unwrap_or(1)
}

/// The value `command` sets explicitly for `key`, if any.
fn explicit_env<'a>(command: &'a Command, key: &str) -> Option<&'a OsStr> {
    command
        .get_envs()
        .find(|(name, _)| *name == OsStr::new(key))
        .and_then(|(_, value)| value)
}

fn declared_role(command: &Command, args: &[&OsStr]) -> Option<SelfSpawnRole> {
    let mp_spawn = crate::runtime_flags::RuntimeFlag::MOLT_MP_SPAWN.name();
    let env_declared = explicit_env(command, mp_spawn) == Some(OsStr::new("1"));
    let argv_declared = args.first() == Some(&OsStr::new("--multiprocessing-fork"));
    (env_declared || argv_declared).then_some(SelfSpawnRole::Multiprocessing)
}

/// The interpreter mode CPython would select for `args`, walking its option
/// grammar (`Python/getopt.c`, `SHORT_OPTS` and `longopts`). Parsing stops at
/// the first operand: a script path, a lone `-`, or the word after `--`.
pub(crate) fn interpreter_invocation(args: &[&OsStr]) -> Option<InterpreterInvocation> {
    let mut index = 0;
    while let Some(arg) = args.get(index) {
        let arg = arg.to_str()?;
        index += 1;
        if !arg.starts_with('-') || arg == "-" || arg == "--" {
            return None;
        }
        if let Some(long) = arg.strip_prefix("--") {
            match long {
                "check-hash-based-pycs" => index += 1,
                "help" | "version" | "help-all" | "help-env" | "help-xoptions" => {}
                _ => return None,
            }
            continue;
        }
        for (offset, option) in arg.char_indices().skip(1) {
            match option {
                'c' => return Some(InterpreterInvocation::Command),
                'm' => return Some(InterpreterInvocation::Module),
                'W' | 'X' => {
                    if offset + 1 == arg.len() {
                        index += 1;
                    }
                    break;
                }
                'b' | 'B' | 'd' | 'E' | 'h' | 'i' | 'I' | 'O' | 'P' | 'q' | 'R' | 's' | 'S'
                | 't' | 'u' | 'v' | 'V' | 'x' | '?' => {}
                _ => return None,
            }
        }
    }
    None
}

/// The files the OS may run for `command`, in its search order. A program with
/// a separator names one file; a bare name is searched on `PATH` (the child's
/// when the command sets it), as `std::process::Command` searches.
fn program_candidates(command: &Command) -> Vec<PathBuf> {
    let program = Path::new(command.get_program());
    if program.components().count() > 1 || program.is_absolute() {
        return vec![relative_to_launch_directory(command, program)];
    }
    let mut directories: Vec<PathBuf> = Vec::new();
    if let Some(path) = explicit_env(command, "PATH") {
        directories.extend(std::env::split_paths(path));
    }
    #[cfg(windows)]
    if let Some(app) = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(Path::to_path_buf))
    {
        directories.push(app);
    }
    if let Some(path) = std::env::var_os("PATH") {
        directories.extend(std::env::split_paths(&path));
    }
    directories
        .into_iter()
        .map(|directory| directory.join(executable_name(program.as_os_str())))
        .collect()
}

/// POSIX resolves a relative program after the child changes directory;
/// Windows resolves it against the parent's directory.
fn relative_to_launch_directory(command: &Command, program: &Path) -> PathBuf {
    if program.is_absolute() {
        return program.to_path_buf();
    }
    #[cfg(unix)]
    if let Some(directory) = command.get_current_dir() {
        return directory.join(program);
    }
    #[cfg(not(unix))]
    let _ = command;
    program.to_path_buf()
}

fn executable_name(program: &OsStr) -> OsString {
    #[cfg(windows)]
    if Path::new(program).extension().is_none() {
        let mut name = program.to_os_string();
        name.push(".exe");
        return name;
    }
    program.to_os_string()
}

#[cfg(test)]
mod tests {
    use super::{InterpreterInvocation, interpreter_invocation, parse_depth};
    use std::ffi::OsStr;

    fn invocation(args: &[&str]) -> Option<InterpreterInvocation> {
        let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        interpreter_invocation(&args)
    }

    #[test]
    fn interpreter_modes_follow_cpython_option_grammar() {
        use InterpreterInvocation::{Command, Module};
        assert_eq!(invocation(&["-c", "print(1)"]), Some(Command));
        assert_eq!(invocation(&["-cprint(1)"]), Some(Command));
        assert_eq!(invocation(&["-m", "pip"]), Some(Module));
        assert_eq!(invocation(&["-u", "-I", "-c", "x"]), Some(Command));
        assert_eq!(invocation(&["-Ic", "x"]), Some(Command));
        assert_eq!(invocation(&["-X", "utf8", "-m", "json.tool"]), Some(Module));
        assert_eq!(invocation(&["-Xutf8", "-c", "x"]), Some(Command));
        assert_eq!(invocation(&["-W", "error", "-c", "x"]), Some(Command));
        assert_eq!(
            invocation(&["--check-hash-based-pycs", "always", "-c", "x"]),
            Some(Command)
        );
        // An option value is not an option: `-X c` sets X option "c".
        assert_eq!(invocation(&["-X", "c", "script.py"]), None);
        assert_eq!(invocation(&["-Wc", "script.py"]), None);
    }

    #[test]
    fn operands_and_unknown_options_end_the_interpreter_grammar() {
        assert_eq!(invocation(&[]), None);
        assert_eq!(invocation(&["script.py", "-c"]), None);
        assert_eq!(invocation(&["-", "-c"]), None);
        assert_eq!(invocation(&["--", "-c"]), None);
        assert_eq!(invocation(&["--multiprocessing-fork"]), None);
        assert_eq!(invocation(&["--worker", "-c"]), None);
        assert_eq!(invocation(&["-J", "-c"]), None);
        assert_eq!(invocation(&["-k", "-c"]), None);
    }

    #[test]
    fn a_malformed_depth_can_only_raise_the_depth() {
        assert_eq!(parse_depth(OsStr::new("0")), 0);
        assert_eq!(parse_depth(OsStr::new("3")), 3);
        assert_eq!(parse_depth(OsStr::new("-1")), 1);
        assert_eq!(parse_depth(OsStr::new("deep")), 1);
    }
}
