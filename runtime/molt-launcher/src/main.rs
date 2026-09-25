//! Native entry point for the release source's single Python bootstrap.

use std::env;
use std::ffi::{OsStr, OsString};
use std::io;
use std::path::PathBuf;
use std::process::Command;

#[cfg(unix)]
use std::os::unix::process::CommandExt;

// The release source is the only authored bootstrap. Embedding it avoids a
// second, mutable executable copy under lib/molt in the installed bundle.
const BOOTSTRAP: &str = include_str!("../../../packaging/bootstrap.py");

#[cfg(windows)]
mod console {
    use std::io;

    const CTRL_C_EVENT: u32 = 0;
    const CTRL_BREAK_EVENT: u32 = 1;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        #[link_name = "SetConsoleCtrlHandler"]
        fn set_console_ctrl_handler(
            handler: Option<unsafe extern "system" fn(u32) -> i32>,
            add: i32,
        ) -> i32;
    }

    // Console control events are delivered to both launcher and child. The
    // child owns interruption semantics; the launcher must remain to wait and
    // return its actual exit status. Custom handlers are not inherited by the
    // spawned Python process.
    unsafe extern "system" fn retain_parent_for_child(event: u32) -> i32 {
        if matches!(event, CTRL_C_EVENT | CTRL_BREAK_EVENT) {
            1
        } else {
            0
        }
    }

    pub fn retain_parent_for_interrupts() -> io::Result<()> {
        if unsafe { set_console_ctrl_handler(Some(retain_parent_for_child), 1) } == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(())
        }
    }
}

fn bundle_root() -> io::Result<PathBuf> {
    // Winget and Homebrew expose a symlink outside the immutable bundle. The
    // executable's canonical path, not argv[0] or the shell cwd, owns layout.
    let executable = env::current_exe()?.canonicalize()?;
    executable
        .parent()
        .and_then(|bin| bin.parent())
        .map(PathBuf::from)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "invalid Molt bundle path"))
}

fn invoke(python: &OsStr, py_launcher: bool, root: &PathBuf, args: &[OsString]) -> io::Result<i32> {
    let mut command = Command::new(python);
    #[cfg(windows)]
    command
        // The Python install manager otherwise installs a runtime on demand.
        // The legacy launcher enables Store installs when these variables are
        // present, even if their value is "0" or "false".
        .env("PYTHON_MANAGER_AUTOMATIC_INSTALL", "false")
        .env_remove("PYLAUNCHER_ALLOW_INSTALL")
        .env_remove("PYLAUNCHER_ALWAYS_INSTALL");
    if py_launcher {
        command.arg("-3");
    }
    command
        .arg("-I")
        .arg("-B")
        .arg("-c")
        .arg(BOOTSTRAP)
        .arg(root)
        .args(args);
    #[cfg(unix)]
    {
        Err(command.exec())
    }
    #[cfg(windows)]
    {
        Ok(command.status()?.code().unwrap_or(1))
    }
}

fn run() -> io::Result<i32> {
    let root = bundle_root()?;
    let args: Vec<OsString> = env::args_os().skip(1).collect();
    #[cfg(windows)]
    console::retain_parent_for_interrupts()?;
    if let Some(python) = env::var_os("PYTHON").filter(|value| !value.is_empty()) {
        return invoke(&python, false, &root, &args);
    }
    // Package managers can bind their exact interpreter without adding a
    // wrapper or relying on a keg-only Python being visible on PATH. Presence
    // is checked without following the link so a broken binding fails loudly.
    let private_python = root.join("libexec").join(if cfg!(windows) {
        "python.exe"
    } else {
        "python"
    });
    match private_python.symlink_metadata() {
        Ok(_) => {
            return invoke(private_python.as_os_str(), false, &root, &args).map_err(|error| {
                io::Error::new(
                    error.kind(),
                    format!(
                        "bound Python interpreter {}: {error}",
                        private_python.display()
                    ),
                )
            });
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    #[cfg(windows)]
    let choices = [("py", true), ("python", false)];
    #[cfg(unix)]
    let choices = [("python3", false), ("python", false)];
    for (python, py_launcher) in choices {
        match invoke(OsStr::new(python), py_launcher, &root, &args) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            outcome => return outcome,
        }
    }
    Err(io::Error::new(
        io::ErrorKind::NotFound,
        "Python 3.12+ not found",
    ))
}

fn main() {
    match run() {
        Ok(code) => std::process::exit(code),
        Err(error) => {
            eprintln!("molt: {error}");
            std::process::exit(1);
        }
    }
}
