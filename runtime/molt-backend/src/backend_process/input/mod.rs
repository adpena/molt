mod cbor;
mod json;
mod msgpack;
mod ndjson;
mod source;

use std::io;

pub(crate) fn read_backend_ir_document(
    ir_format: &str,
    ir_file_path: Option<&str>,
) -> io::Result<molt_backend::BackendIrDocument> {
    match ir_format {
        "msgpack" => msgpack::read_msgpack_backend_ir_document(ir_file_path),
        "cbor" => cbor::read_cbor_backend_ir_document(ir_file_path),
        "ndjson" => ndjson::read_ndjson_backend_ir_document(ir_file_path),
        _ => json::read_json_backend_ir_document(ir_file_path),
    }
}
