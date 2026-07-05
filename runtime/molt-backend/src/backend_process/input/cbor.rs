use std::io;

#[cfg(feature = "cbor")]
use super::source::{invalid_ir_exit, open_ir_file};
#[cfg(feature = "cbor")]
use crate::backend_process::io_limits::{read_bounded_request_bytes, stdin_request_limit_bytes};

#[cfg(not(feature = "cbor"))]
pub(super) fn read_cbor_backend_ir_document(
    _ir_file_path: Option<&str>,
) -> io::Result<molt_backend::BackendIrDocument> {
    eprintln!("CBOR support requires the 'cbor' feature");
    std::process::exit(1);
}

#[cfg(feature = "cbor")]
pub(super) fn read_cbor_backend_ir_document(
    ir_file_path: Option<&str>,
) -> io::Result<molt_backend::BackendIrDocument> {
    if let Some(ir_path) = ir_file_path {
        let file = open_ir_file(ir_path)?;
        let reader = io::BufReader::new(file);
        return match ciborium::de::from_reader::<molt_backend::BackendIrDocument, _>(reader) {
            Ok(ir) => Ok(ir),
            Err(err) => invalid_ir_exit("CBOR IR", err),
        };
    }

    let buf = read_bounded_request_bytes(
        io::stdin().lock(),
        stdin_request_limit_bytes(),
        "backend stdin request",
    )?;
    let result = ciborium::de::from_reader::<molt_backend::BackendIrDocument, _>(&buf[..]);
    drop(buf);
    match result {
        Ok(ir) => Ok(ir),
        Err(err) => invalid_ir_exit("CBOR IR", err),
    }
}
