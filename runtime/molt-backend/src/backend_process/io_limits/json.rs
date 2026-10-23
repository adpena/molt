use std::io;
use std::path::Path;

use molt_artifact_publish::write_atomically;

#[cfg(feature = "native-backend")]
pub(crate) fn write_json_artifact<T: serde::Serialize>(path: &Path, value: &T) -> io::Result<()> {
    write_atomically(path, |writer| {
        serde_json::to_writer(writer, value).map_err(io::Error::other)
    })
}
