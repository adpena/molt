use std::io;

use super::content::{ContentIdentity, ContentReader};
use super::source::{invalid_ir, open_ir_file};
use crate::backend_process::io_limits::{RequestBoundedRead, stdin_request_limit_bytes};

pub(super) fn decode(reader: &mut impl io::Read) -> io::Result<molt_backend::BackendIrDocument> {
    molt_backend::BackendIrDocument::from_msgpack_reader(reader)
        .map_err(|err| invalid_ir("msgpack IR", err))
}

pub(super) fn read_msgpack_backend_ir_document(
    ir_file_path: Option<&str>,
) -> io::Result<molt_backend::BackendIrDocument> {
    read_captured_msgpack(ir_file_path).map(|(document, _)| document)
}

pub(crate) fn read_captured_msgpack(
    ir_file_path: Option<&str>,
) -> io::Result<(molt_backend::BackendIrDocument, ContentIdentity)> {
    fn captured(
        reader: impl io::Read,
        capacity: usize,
    ) -> io::Result<(molt_backend::BackendIrDocument, ContentIdentity)> {
        let mut reader = ContentReader::new(reader);
        // Hash filled blocks, not individual decoder reads. Successful decode
        // requires EOF, so every captured byte belongs to the complete frame.
        let document = decode(&mut io::BufReader::with_capacity(capacity, &mut reader))?;
        Ok((document, reader.identity()))
    }
    if let Some(ir_path) = ir_file_path {
        return captured(open_ir_file(ir_path)?, 8192);
    }
    let stdin = io::stdin();
    let bounded = RequestBoundedRead::new(
        stdin.lock(),
        stdin_request_limit_bytes(),
        "backend stdin request",
    );
    captured(bounded, 1 << 20)
}
