use std::fs::File;
use std::io;
use std::path::Path;

use crate::backend_process::write_atomically;

#[cfg(feature = "native-backend")]
pub(crate) fn write_json_artifact<T: serde::Serialize>(path: &Path, value: &T) -> io::Result<()> {
    write_atomically(path, |writer| {
        serde_json::to_writer(writer, value).map_err(io::Error::other)
    })
}

#[cfg(feature = "native-backend")]
pub(crate) fn read_json_artifact<T: serde::de::DeserializeOwned>(
    path: &Path,
    label: &str,
) -> io::Result<T> {
    let file = File::open(path).map_err(|err| {
        io::Error::new(
            err.kind(),
            format!("failed to open {label} '{}': {err}", path.display()),
        )
    })?;
    let reader = io::BufReader::new(file);
    serde_json::from_reader(reader).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("invalid {label} '{}': {err}", path.display()),
        )
    })
}
