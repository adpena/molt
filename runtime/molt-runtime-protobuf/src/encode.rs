//! Schema-driven protobuf message encoding.

use crate::{
    encode_bytes_field, encode_tag, encode_uint64_field, encode_varint, FieldDef, MessageSchema,
    WireType,
};

/// Runtime value for a single protobuf field.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldValue {
    Uint64(u64),
    Int64(i64),
    Fixed32(u32),
    Fixed64(u64),
    Bytes(Vec<u8>),
}

/// Errors that can occur while encoding a protobuf message.
#[derive(Debug)]
pub enum MessageEncodeError {
    /// The number of values provided does not match the number of fields in the schema.
    FieldCountMismatch { schema_fields: usize, values: usize },
    /// A field value's type is incompatible with the schema's declared wire type.
    WireTypeMismatch {
        field_name: String,
        wire_type: crate::WireType,
        value: String,
    },
}

impl std::fmt::Display for MessageEncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageEncodeError::FieldCountMismatch {
                schema_fields,
                values,
            } => write!(
                f,
                "encode_message: schema has {schema_fields} fields but {values} values were provided",
            ),
            MessageEncodeError::WireTypeMismatch {
                field_name,
                wire_type,
                value,
            } => write!(
                f,
                "encode_field: incompatible wire type {wire_type:?} for value {value} on field '{field_name}'",
            ),
        }
    }
}

impl std::error::Error for MessageEncodeError {}

/// Encode a message according to its schema and field values.
///
/// `values` must be in the same order as `schema.fields`.
/// Returns `Err(MessageEncodeError::FieldCountMismatch)` if `values.len() != schema.fields.len()`.
pub fn encode_message(
    schema: &MessageSchema,
    values: &[FieldValue],
) -> Result<Vec<u8>, MessageEncodeError> {
    if schema.fields.len() != values.len() {
        return Err(MessageEncodeError::FieldCountMismatch {
            schema_fields: schema.fields.len(),
            values: values.len(),
        });
    }

    let mut buf = Vec::new();
    for (field, value) in schema.fields.iter().zip(values.iter()) {
        encode_field(field, value, &mut buf)?;
    }
    Ok(buf)
}

fn encode_field(
    field: &FieldDef,
    value: &FieldValue,
    buf: &mut Vec<u8>,
) -> Result<(), MessageEncodeError> {
    match (field.wire_type, value) {
        (WireType::Varint, FieldValue::Uint64(v)) => {
            encode_uint64_field(field.number, *v, buf);
        }
        (WireType::Varint, FieldValue::Int64(v)) => {
            // Protobuf encodes signed integers as unsigned varints (two's complement).
            encode_uint64_field(field.number, *v as u64, buf);
        }
        (WireType::Fixed64, FieldValue::Fixed64(v)) => {
            encode_tag(field.number, WireType::Fixed64, buf);
            buf.extend_from_slice(&v.to_le_bytes());
        }
        (WireType::Fixed32, FieldValue::Fixed32(v)) => {
            encode_tag(field.number, WireType::Fixed32, buf);
            buf.extend_from_slice(&v.to_le_bytes());
        }
        (WireType::LengthDelimited, FieldValue::Bytes(v)) => {
            encode_bytes_field(field.number, v, buf);
        }
        (wire_type, value) => {
            // Encode with best-effort type coercion for mismatched but compatible types.
            match (wire_type, value) {
                (WireType::Varint, FieldValue::Fixed32(v)) => {
                    encode_uint64_field(field.number, u64::from(*v), buf);
                }
                (WireType::Varint, FieldValue::Fixed64(v)) => {
                    encode_uint64_field(field.number, *v, buf);
                }
                (WireType::Fixed64, FieldValue::Uint64(v)) => {
                    encode_tag(field.number, WireType::Fixed64, buf);
                    buf.extend_from_slice(&v.to_le_bytes());
                }
                (WireType::Fixed64, FieldValue::Int64(v)) => {
                    encode_tag(field.number, WireType::Fixed64, buf);
                    buf.extend_from_slice(&(*v as u64).to_le_bytes());
                }
                (WireType::Fixed32, FieldValue::Uint64(v)) => {
                    encode_tag(field.number, WireType::Fixed32, buf);
                    buf.extend_from_slice(&(*v as u32).to_le_bytes());
                }
                (WireType::LengthDelimited, FieldValue::Uint64(v)) => {
                    // Encode the varint representation as length-delimited bytes.
                    let mut tmp = Vec::new();
                    encode_varint(*v, &mut tmp);
                    encode_bytes_field(field.number, &tmp, buf);
                }
                _ => {
                    return Err(MessageEncodeError::WireTypeMismatch {
                        field_name: field.name.clone(),
                        wire_type,
                        value: format!("{value:?}"),
                    });
                }
            }
        }
    }
    Ok(())
}
