use std::io;

use super::source::{invalid_ir, open_ir_file};
use crate::backend_process::io_limits::{read_bounded_request_bytes, stdin_request_limit_bytes};

pub(crate) fn decode(reader: impl io::Read) -> io::Result<molt_backend::BackendIrDocument> {
    serde_json::from_reader(reader).map_err(|err| invalid_ir("IR JSON", err))
}

pub(super) fn read_json_backend_ir_document(
    ir_file_path: Option<&str>,
) -> io::Result<molt_backend::BackendIrDocument> {
    if let Some(ir_path) = ir_file_path {
        let file = open_ir_file(ir_path)?;
        let reader = io::BufReader::with_capacity(1 << 20, file);
        return decode(reader);
    }

    let raw_bytes = read_bounded_request_bytes(
        io::stdin().lock(),
        stdin_request_limit_bytes(),
        "backend stdin request",
    )?;
    let buffer = String::from_utf8(raw_bytes).map_err(|err| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("backend stdin request is not UTF-8: {err}"),
        )
    })?;
    let result = serde_json::from_str::<molt_backend::BackendIrDocument>(&buffer);
    drop(buffer);
    result.map_err(|err| invalid_ir("IR JSON", err))
}
