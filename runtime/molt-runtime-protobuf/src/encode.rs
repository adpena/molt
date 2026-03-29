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

/// Encode a message according to its schema and field values.
///
/// `values` must be in the same order as `schema.fields`.
/// Panics if `values.len() != schema.fields.len()`.
pub fn encode_message(schema: &MessageSchema, values: &[FieldValue]) -> Vec<u8> {
    assert_eq!(
        schema.fields.len(),
        values.len(),
        "encode_message: schema has {} fields but {} values were provided",
        schema.fields.len(),
        values.len(),
    );

    let mut buf = Vec::new();
    for (field, value) in schema.fields.iter().zip(values.iter()) {
        encode_field(field, value, &mut buf);
    }
    buf
}

fn encode_field(field: &FieldDef, value: &FieldValue, buf: &mut Vec<u8>) {
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
                _ => panic!(
                    "encode_field: incompatible wire type {:?} for value {:?} on field '{}'",
                    wire_type, value, field.name,
                ),
            }
        }
    }
}
