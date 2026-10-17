use std::io;

use super::source::{invalid_ir, open_ir_file};
use crate::backend_process::io_limits::{RequestBoundedRead, stdin_request_limit_bytes};

pub(super) fn read_ndjson_backend_ir_document(
    ir_file_path: Option<&str>,
) -> io::Result<molt_backend::BackendIrDocument> {
    if let Some(ir_path) = ir_file_path {
        let file = open_ir_file(ir_path)?;
        let reader = io::BufReader::new(file);
        return molt_backend::BackendIrDocument::from_ndjson_reader(reader)
            .map_err(|err| invalid_ir("NDJSON IR", err));
    }

    let stdin = io::stdin();
    let bounded = RequestBoundedRead::new(
        stdin.lock(),
        stdin_request_limit_bytes(),
        "backend stdin request",
    );
    let reader = io::BufReader::new(bounded);
    molt_backend::BackendIrDocument::from_ndjson_reader(reader)
        .map_err(|err| invalid_ir("NDJSON IR", err))
}
