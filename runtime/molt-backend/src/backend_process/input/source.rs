use std::io;

pub(super) fn open_ir_file(ir_path: &str) -> io::Result<std::fs::File> {
    std::fs::File::open(ir_path)
        .map_err(|err| io::Error::other(format!("failed to open IR file '{ir_path}': {err}")))
}

pub(super) fn invalid_ir(context: &str, err: impl std::fmt::Display) -> io::Error {
    io::Error::new(
        io::ErrorKind::InvalidData,
        format!("invalid {context}: {err}"),
    )
}

/// Check the same reader used by the decoder, including any buffered suffix.
pub(super) fn require_eof(reader: &mut impl io::Read, context: &str) -> io::Result<()> {
    let mut suffix = [0_u8; 1];
    loop {
        match reader.read(&mut suffix) {
            Ok(0) => return Ok(()),
            Ok(_) => return Err(invalid_ir(context, "trailing bytes after the IR value")),
            Err(err) if err.kind() == io::ErrorKind::Interrupted => continue,
            Err(err) => return Err(err),
        }
    }
}
