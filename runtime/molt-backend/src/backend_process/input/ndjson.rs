use std::io;

use super::source::{invalid_ir_exit, open_ir_file};
use crate::backend_process::io_limits::{RequestBoundedRead, stdin_request_limit_bytes};

pub(super) fn read_ndjson_backend_ir_document(
    ir_file_path: Option<&str>,
) -> io::Result<molt_backend::BackendIrDocument> {
    if let Some(ir_path) = ir_file_path {
        let file = open_ir_file(ir_path)?;
        let reader = io::BufReader::new(file);
        return match molt_backend::BackendIrDocument::from_ndjson_reader(reader) {
            Ok(ir) => Ok(ir),
            Err(err) => invalid_ir_exit("NDJSON IR", err),
        };
    }

    let stdin = io::stdin();
    let bounded = RequestBoundedRead::new(
        stdin.lock(),
        stdin_request_limit_bytes(),
        "backend stdin request",
    );
    let reader = io::BufReader::new(bounded);
    match molt_backend::BackendIrDocument::from_ndjson_reader(reader) {
        Ok(ir) => Ok(ir),
        Err(err) => invalid_ir_exit("NDJSON IR", err),
    }
}
