use std::io;

use super::source::{invalid_ir, open_ir_file, require_eof};
use crate::backend_process::io_limits::{RequestBoundedRead, stdin_request_limit_bytes};

pub(super) fn decode(reader: &mut impl io::Read) -> io::Result<molt_backend::BackendIrDocument> {
    let document =
        rmp_serde::from_read(&mut *reader).map_err(|err| invalid_ir("msgpack IR", err))?;
    require_eof(reader, "msgpack IR")?;
    Ok(document)
}

pub(super) fn read_msgpack_backend_ir_document(
    ir_file_path: Option<&str>,
) -> io::Result<molt_backend::BackendIrDocument> {
    if let Some(ir_path) = ir_file_path {
        let file = open_ir_file(ir_path)?;
        return decode(&mut io::BufReader::new(file));
    }

    let stdin = io::stdin();
    let bounded = RequestBoundedRead::new(
        stdin.lock(),
        stdin_request_limit_bytes(),
        "backend stdin request",
    );
    decode(&mut io::BufReader::with_capacity(1 << 20, bounded))
}
