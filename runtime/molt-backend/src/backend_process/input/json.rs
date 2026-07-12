use std::io;

use super::source::{invalid_ir_exit, open_ir_file};
use crate::backend_process::io_limits::{read_bounded_request_bytes, stdin_request_limit_bytes};

pub(super) fn read_json_backend_ir_document(
    ir_file_path: Option<&str>,
) -> io::Result<molt_backend::BackendIrDocument> {
    if let Some(ir_path) = ir_file_path {
        let file = open_ir_file(ir_path)?;
        let reader = io::BufReader::with_capacity(1 << 20, file);
        return match serde_json::from_reader::<_, molt_backend::BackendIrDocument>(reader) {
            Ok(ir) => Ok(ir),
            Err(err) => invalid_ir_exit("IR JSON", err),
        };
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
    match result {
        Ok(ir) => Ok(ir),
        Err(err) => invalid_ir_exit("IR JSON", err),
    }
}
