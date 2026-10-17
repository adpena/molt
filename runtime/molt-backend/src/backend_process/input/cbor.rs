use std::io;

#[cfg(feature = "cbor")]
use super::source::{invalid_ir, open_ir_file, require_eof};
#[cfg(feature = "cbor")]
use crate::backend_process::io_limits::{read_bounded_request_bytes, stdin_request_limit_bytes};

#[cfg(not(feature = "cbor"))]
pub(super) fn read_cbor_backend_ir_document(
    _ir_file_path: Option<&str>,
) -> io::Result<molt_backend::BackendIrDocument> {
    Err(io::Error::new(
        io::ErrorKind::Unsupported,
        "CBOR support requires the 'cbor' feature",
    ))
}

#[cfg(feature = "cbor")]
pub(super) fn decode(reader: &mut impl io::Read) -> io::Result<molt_backend::BackendIrDocument> {
    let document =
        ciborium::de::from_reader(&mut *reader).map_err(|err| invalid_ir("CBOR IR", err))?;
    require_eof(reader, "CBOR IR")?;
    Ok(document)
}

#[cfg(feature = "cbor")]
pub(super) fn read_cbor_backend_ir_document(
    ir_file_path: Option<&str>,
) -> io::Result<molt_backend::BackendIrDocument> {
    if let Some(ir_path) = ir_file_path {
        let file = open_ir_file(ir_path)?;
        return decode(&mut io::BufReader::new(file));
    }

    let buf = read_bounded_request_bytes(
        io::stdin().lock(),
        stdin_request_limit_bytes(),
        "backend stdin request",
    )?;
    decode(&mut &buf[..])
}
