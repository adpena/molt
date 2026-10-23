mod cbor;
#[cfg(any(feature = "native-backend", molt_backend_daemon))]
pub(crate) mod content;
#[cfg(any(feature = "native-backend", molt_backend_daemon))]
pub(crate) use content::ContentIdentity;
mod json;
mod msgpack;
mod ndjson;
mod source;

#[cfg(molt_backend_daemon)]
pub(crate) use source::open_ir_file;

#[cfg(test)]
mod tests;

use std::io;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum IrFormat {
    Json,
    MessagePack,
    Cbor,
    Ndjson,
}

impl IrFormat {
    pub(crate) fn parse(value: &str) -> io::Result<Self> {
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
    ir_format: IrFormat,
    ir_file_path: Option<&str>,
) -> io::Result<molt_backend::BackendIrDocument> {
    match ir_format {
        IrFormat::Json => json::read_json_backend_ir_document(ir_file_path),
        IrFormat::MessagePack => msgpack::read_msgpack_backend_ir_document(ir_file_path),
        IrFormat::Cbor => cbor::read_cbor_backend_ir_document(ir_file_path),
        IrFormat::Ndjson => ndjson::read_ndjson_backend_ir_document(ir_file_path),
    }
}
