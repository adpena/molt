mod cbor;
mod json;
mod msgpack;
mod ndjson;
mod source;

#[cfg(test)]
mod tests;

use std::io;

#[cfg(molt_backend_daemon)]
pub(crate) use json::decode as decode_json_ir_document;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum IrFormat {
    Json,
    MessagePack,
    Cbor,
    Ndjson,
}

impl IrFormat {
    fn parse(value: &str) -> io::Result<Self> {
        match value {
            "json" => Ok(Self::Json),
            "msgpack" => Ok(Self::MessagePack),
            "cbor" => Ok(Self::Cbor),
            "ndjson" => Ok(Self::Ndjson),
            _ => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("unknown IR format {value:?}; expected json, msgpack, cbor, or ndjson"),
            )),
        }
    }
}

pub(crate) fn read_backend_ir_document(
    ir_format: &str,
    ir_file_path: Option<&str>,
) -> io::Result<molt_backend::BackendIrDocument> {
    match IrFormat::parse(ir_format)? {
        IrFormat::Json => json::read_json_backend_ir_document(ir_file_path),
        IrFormat::MessagePack => msgpack::read_msgpack_backend_ir_document(ir_file_path),
        IrFormat::Cbor => cbor::read_cbor_backend_ir_document(ir_file_path),
        IrFormat::Ndjson => ndjson::read_ndjson_backend_ir_document(ir_file_path),
    }
}
