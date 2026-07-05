use std::io;
use std::path::{Path, PathBuf};

use super::super::io_limits::ensure_output_parent_dir;

pub(crate) fn relocatable_linker_binary(linker_override: Option<&str>) -> String {
    linker_override
        .filter(|value| !value.trim().is_empty())
        .map(str::to_string)
        .or_else(|| std::env::var("MOLT_LINKER").ok())
        .or_else(|| std::env::var("LD").ok())
        .or_else(|| std::env::var("CC").ok())
        .unwrap_or_else(|| "ld".to_string())
}

pub(crate) fn merge_relocatable_objects(
    output_path: &Path,
    object_paths: &[PathBuf],
    linker_override: Option<&str>,
) -> io::Result<()> {
    if object_paths.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "no object files to merge",
        ));
    }

    ensure_output_parent_dir(output_path.to_str().unwrap_or_default())?;

    if object_paths.len() == 1 {
        std::fs::copy(&object_paths[0], output_path).map_err(|err| {
            io::Error::new(
                err.kind(),
                format!(
                    "failed to copy batch object '{}' to '{}': {}",
                    object_paths[0].display(),
                    output_path.display(),
                    err
                ),
            )
        })?;
        return Ok(());
    }

    let ld_bin = relocatable_linker_binary(linker_override);
    let mut cmd = std::process::Command::new(&ld_bin);
    if ld_bin.contains("clang") || ld_bin.contains("gcc") {
        cmd.arg("-Wl,-r").arg("-o").arg(output_path);
    } else {
        cmd.arg("-r").arg("-o").arg(output_path);
    }
    for path in object_paths {
        cmd.arg(path);
    }
    let merge_output = cmd.output().map_err(|err| {
        io::Error::other(format!(
            "failed to run relocatable linker '{ld_bin}' for '{}': {err}",
            output_path.display()
        ))
    })?;
    if merge_output.status.success() {
        return Ok(());
    }

    let stderr = String::from_utf8_lossy(&merge_output.stderr)
        .trim()
        .to_string();
    let detail = if stderr.is_empty() {
        format!("exit {}", merge_output.status)
    } else {
        stderr
    };
    Err(io::Error::other(format!(
        "relocatable link failed via '{ld_bin}' for '{}': {detail}",
        output_path.display()
    )))
}
