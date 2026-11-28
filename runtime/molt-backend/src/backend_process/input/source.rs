use std::io;

pub(crate) fn open_ir_file(ir_path: &str) -> io::Result<std::fs::File> {
    molt_artifact_publish::open_regular_artifact(std::path::Path::new(ir_path))
        .map_err(|err| io::Error::other(format!("failed to open IR file '{ir_path}': {err}")))
}

pub(super) fn invalid_ir(context: &str, err: impl std::fmt::Display) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("invalid {context}: {err}"),
    )
}

/// Check the same reader used by the decoder, including any buffered suffix.
#[cfg(feature = "cbor")]
pub(super) fn require_eof(reader: &mut impl io::Read, context: &str) -> io::Result<()> {
    if !molt_ir::ir::ir_frame_at_eof(reader)? {
        return Err(invalid_ir(context, "trailing bytes after the IR value"));
    }
    Ok(())
}
