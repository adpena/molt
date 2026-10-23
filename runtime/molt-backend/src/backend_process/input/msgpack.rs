use std::io;

use super::source::{invalid_ir, open_ir_file};
use crate::backend_process::io_limits::{RequestBoundedRead, stdin_request_limit_bytes};

pub(super) fn decode(reader: &mut impl io::Read) -> io::Result<molt_backend::BackendIrDocument> {
    molt_backend::BackendIrDocument::from_msgpack_reader(reader)
        .map_err(|err| invalid_ir("msgpack IR", err))
}

pub(super) fn read_msgpack_backend_ir_document(
    ir_file_path: Option<&str>,
) -> io::Result<molt_backend::BackendIrDocument> {
    if let Some(ir_path) = ir_file_path {
        return decode(&mut io::BufReader::with_capacity(
            8192,
            open_ir_file(ir_path)?,
        ));
    }
    let stdin = io::stdin();
    let bounded = RequestBoundedRead::new(
        stdin.lock(),
        stdin_request_limit_bytes(),
        "backend stdin request",
    );
    decode(&mut io::BufReader::with_capacity(1 << 20, bounded))
}
