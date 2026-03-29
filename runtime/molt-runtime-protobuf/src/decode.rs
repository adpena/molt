//! Schema-driven protobuf message decoding.

use crate::encode::FieldValue;
use crate::{DecodeError, MessageSchema, WireType};

/// Errors that can occur while decoding a protobuf message.
#[derive(Debug)]
pub enum MessageDecodeError {
    /// The input data was truncated before a complete field could be read.
    Truncated { context: &'static str },
    /// A field number was encountered that does not appear in the schema.
    UnknownField { number: u32 },
    /// The wire type on the wire did not match the schema's expected wire type.
    WireTypeMismatch {
        field: String,
        expected: WireType,
    },
    /// A varint could not be decoded.
    Varint(DecodeError),
}

impl std::fmt::Display for MessageDecodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            MessageDecodeError::Truncated { context } => {
                write!(f, "truncated protobuf data: {context}")
            }
            MessageDecodeError::UnknownField { number } => {
                write!(f, "unknown field number {number}")
            }
            MessageDecodeError::WireTypeMismatch { field, expected } => {
                write!(
                    f,
                    "wire type mismatch for field '{field}': expected {expected:?}"
                )
            }
            MessageDecodeError::Varint(e) => write!(f, "varint decode error: {e}"),
        }
    }
}

impl std::error::Error for MessageDecodeError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            MessageDecodeError::Varint(e) => Some(e),
            _ => None,
        }
    }
}

impl From<DecodeError> for MessageDecodeError {
    fn from(e: DecodeError) -> Self {
        MessageDecodeError::Varint(e)
    }
}

/// Decode a protobuf message according to its schema.
///
/// Returns field values in schema field order.  Fields present on the wire
/// but absent from the schema are skipped.  Schema fields not present on
/// the wire receive a default zero-value.
pub fn decode_message(
    schema: &MessageSchema,
    data: &[u8],
) -> Result<Vec<FieldValue>, MessageDecodeError> {
    // Pre-allocate result slots with default values.
    let mut results: Vec<Option<FieldValue>> = vec![None; schema.fields.len()];

    // Build a lookup from field number to schema index.
    let field_index: std::collections::HashMap<u32, usize> = schema
        .fields
        .iter()
        .enumerate()
        .map(|(i, f)| (f.number, i))
        .collect();

    let mut pos = 0;
    while pos < data.len() {
        // Decode the tag (field number + wire type).
        let (tag_varint, tag_len) = decode_varint_at(data, pos, "tag")?;
        pos += tag_len;

        let wire_type_raw = (tag_varint & 0x07) as u32;
        let field_number = (tag_varint >> 3) as u32;

        if let Some(&idx) = field_index.get(&field_number) {
            let field_def = &schema.fields[idx];
            let expected_wire = wire_type_id(field_def.wire_type);

            if wire_type_raw != expected_wire {
                return Err(MessageDecodeError::WireTypeMismatch {
                    field: field_def.name.clone(),
                    expected: field_def.wire_type,
                });
            }

            let (value, consumed) = decode_field_value(field_def.wire_type, data, pos)?;
            pos += consumed;
            results[idx] = Some(value);
        } else {
            // Skip unknown field.
            let consumed = skip_wire_value(wire_type_raw, data, pos)?;
            pos += consumed;
        }
    }

    // Fill in defaults for missing fields.
    Ok(results
        .into_iter()
        .enumerate()
        .map(|(i, opt)| {
            opt.unwrap_or_else(|| default_value(schema.fields[i].wire_type))
        })
        .collect())
}

/// Decode a single field value from the given wire type.
fn decode_field_value(
    wire_type: WireType,
    data: &[u8],
    pos: usize,
) -> Result<(FieldValue, usize), MessageDecodeError> {
    match wire_type {
        WireType::Varint => {
            let (val, len) = decode_varint_at(data, pos, "varint field")?;
            Ok((FieldValue::Uint64(val), len))
        }
        WireType::Fixed64 => {
            if pos + 8 > data.len() {
                return Err(MessageDecodeError::Truncated {
                    context: "fixed64 field",
                });
            }
            let val = u64::from_le_bytes(data[pos..pos + 8].try_into().unwrap());
            Ok((FieldValue::Fixed64(val), 8))
        }
        WireType::Fixed32 => {
            if pos + 4 > data.len() {
                return Err(MessageDecodeError::Truncated {
                    context: "fixed32 field",
                });
            }
            let val = u32::from_le_bytes(data[pos..pos + 4].try_into().unwrap());
            Ok((FieldValue::Fixed32(val), 4))
        }
        WireType::LengthDelimited => {
            let (length, len_bytes) = decode_varint_at(data, pos, "length prefix")?;
            let length = length as usize;
            let start = pos + len_bytes;
            if start + length > data.len() {
                return Err(MessageDecodeError::Truncated {
                    context: "length-delimited field",
                });
            }
            let payload = data[start..start + length].to_vec();
            Ok((FieldValue::Bytes(payload), len_bytes + length))
        }
    }
}

/// Skip an unknown field value based on its raw wire type ID.
fn skip_wire_value(
    wire_type_raw: u32,
    data: &[u8],
    pos: usize,
) -> Result<usize, MessageDecodeError> {
    match wire_type_raw {
        0 => {
            // Varint
            let (_, len) = decode_varint_at(data, pos, "skip varint")?;
            Ok(len)
        }
        1 => {
            // 64-bit
            if pos + 8 > data.len() {
                return Err(MessageDecodeError::Truncated {
                    context: "skip fixed64",
                });
            }
            Ok(8)
        }
        2 => {
            // Length-delimited
            let (length, len_bytes) = decode_varint_at(data, pos, "skip length prefix")?;
            let total = len_bytes + length as usize;
            if pos + total > data.len() {
                return Err(MessageDecodeError::Truncated {
                    context: "skip length-delimited",
                });
            }
            Ok(total)
        }
        5 => {
            // 32-bit
            if pos + 4 > data.len() {
                return Err(MessageDecodeError::Truncated {
                    context: "skip fixed32",
                });
            }
            Ok(4)
        }
        _ => Err(MessageDecodeError::Truncated {
            context: "unknown wire type",
        }),
    }
}

/// Decode a varint starting at `data[pos..]`.
fn decode_varint_at(
    data: &[u8],
    pos: usize,
    context: &'static str,
) -> Result<(u64, usize), MessageDecodeError> {
    if pos >= data.len() {
        return Err(MessageDecodeError::Truncated { context });
    }
    crate::decode_varint(&data[pos..]).map_err(MessageDecodeError::Varint)
}

/// Return the wire type ID for our `WireType` enum.
fn wire_type_id(wt: WireType) -> u32 {
    match wt {
        WireType::Varint => 0,
        WireType::Fixed64 => 1,
        WireType::LengthDelimited => 2,
        WireType::Fixed32 => 5,
    }
}

/// Return a default (zero) value for a wire type.
fn default_value(wt: WireType) -> FieldValue {
    match wt {
        WireType::Varint => FieldValue::Uint64(0),
        WireType::Fixed64 => FieldValue::Fixed64(0),
        WireType::Fixed32 => FieldValue::Fixed32(0),
        WireType::LengthDelimited => FieldValue::Bytes(Vec::new()),
    }
}
