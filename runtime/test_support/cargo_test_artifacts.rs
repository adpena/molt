// Shared placement for artifacts compiled by Rust tests. Cargo target custody,
// not a test-scope destructor, owns retention and eventual retirement. Keeping
// generated inputs and images permits content-bound receipt capture and replay.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

pub struct CargoTestArtifacts {
    path: PathBuf,
}

impl CargoTestArtifacts {
    pub fn new(label: &str) -> io::Result<Self> {
        if label.is_empty()
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "artifact label must be a nonempty portable path component",
            ));
        }
        // Cargo resolved the invocation's target into this actual test image.
        // Reinterpreting inherited CARGO_TARGET_DIR here would use the package
        // test cwd, not Cargo's invocation cwd, and duplicate that authority.
        // In supervised execution, the existing supervisor independently owns
        // admission under the captured absolute, exclusive Cargo target.
        let executable = std::env::current_exe()?.canonicalize()?;
        let parent = executable.parent().ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "test executable has no output parent",
            )
        })?;
        let root = parent.join("molt-test-artifacts");
        std::fs::create_dir_all(&root)?;
        let metadata = std::fs::symlink_metadata(&root)?;
        let redirected = metadata.file_type().is_symlink();
        #[cfg(windows)]
        let redirected = redirected || {
            use std::os::windows::fs::MetadataExt;
            // Junctions and other reparse points are redirections too.
            const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x400;
            metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        };
        if redirected || !metadata.is_dir() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Cargo test artifact root must be a real directory, not a redirect",
            ));
        }
        let root = root.canonicalize()?;
        if root.parent() != Some(parent) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "Cargo test artifact root escaped its test image's output parent",
            ));
        }
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let sequence = NEXT
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |value| {
                    value.checked_add(1)
                })
                .map_err(|_| io::Error::other("Cargo test artifact sequence exhausted"))?;
            let path = root.join(format!("{label}-{}-{sequence}", std::process::id()));
            match std::fs::create_dir(&path) {
                Ok(()) => return Ok(Self { path }),
                // Another module or preserved prior run owns this name.
                // Exclusive creation never reuses or erases its contents.
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Preserve canonical filesystem custody, but do not transport Windows
    /// device paths through compiler/linker argument parsers. Child tools work
    /// in the owned directory and receive lossless, explicitly relative paths.
    /// Resolve an explicitly relative tool before changing its working directory;
    /// a bare executable name retains the ordinary PATH lookup contract.
    pub fn command(&self, program: impl AsRef<OsStr>) -> io::Result<Command> {
        let program = Path::new(program.as_ref());
        let mut parts = program.components();
        let bare_name =
            matches!(parts.next(), Some(Component::Normal(_))) && parts.next().is_none();
        let program = if !program.is_absolute() && !bare_name {
            std::path::absolute(program)?
        } else {
            program.to_path_buf()
        };
        let mut command = Command::new(program);
        command.current_dir(&self.path);
        Ok(command)
    }

    /// Prefix an artifact path without formatting through UTF-8 or inserting
    /// shell quotes. Only descendants of this exact owner can be transported;
    /// `./` also keeps a filename beginning with `-` or `@` out of option and
    /// response-file syntax. The file may be an output that does not exist yet.
    pub fn argument(&self, prefix: &str, path: &Path) -> io::Result<OsString> {
        let relative = path.strip_prefix(&self.path).map_err(|_| {
            io::Error::new(
                io::ErrorKind::InvalidInput,
                "artifact argument is outside its owner",
            )
        })?;
        if relative.as_os_str().is_empty()
            || !relative
                .components()
                .all(|part| matches!(part, Component::Normal(_)))
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "artifact argument must be a descendant without traversal",
            ));
        }
        let mut argument = OsString::from(prefix);
        argument.push(Path::new(".").join(relative));
        Ok(argument)
    }
}
