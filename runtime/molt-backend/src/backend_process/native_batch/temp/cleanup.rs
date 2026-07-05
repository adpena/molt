use std::io;
use std::path::Path;

pub(crate) fn remove_native_batch_temp_dir(path: &Path, label: &str) -> io::Result<()> {
    match std::fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(err) => Err(io::Error::new(
            err.kind(),
            format!("failed to remove {label} '{}': {err}", path.display()),
        )),
    }
}

pub(crate) fn finish_native_batch_temp_dir(
    path: &Path,
    label: &str,
    compile_result: io::Result<()>,
) -> io::Result<()> {
    let cleanup_result = remove_native_batch_temp_dir(path, label);
    match (compile_result, cleanup_result) {
        (Ok(()), Ok(())) => Ok(()),
        (Ok(()), Err(cleanup_err)) => Err(cleanup_err),
        (Err(compile_err), Ok(())) => Err(compile_err),
        (Err(compile_err), Err(cleanup_err)) => {
            eprintln!(
                "MOLT_BACKEND: failed to clean {label} '{}' after compile error: {cleanup_err}",
                path.display()
            );
            Err(compile_err)
        }
    }
}
