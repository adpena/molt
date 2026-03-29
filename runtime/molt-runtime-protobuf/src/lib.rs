//! Protobuf serialization for Molt-compiled Python programs.
//!
//! This crate bridges Molt's runtime with the `buffa` protobuf library,
//! providing protobuf encoding and decoding primitives that the compiled
//! runtime can call.
//!
//! # Architecture
//!
//! ```text
//! Python code:     msg = MyMessage(name="hello", value=42)
//!                  wire = protobuf.encode(msg)
//!                  decoded = protobuf.decode(MyMessage, wire)
//!                            │
//! Molt frontend:   Recognizes @molt.proto decorator, generates typed IR
//!                            │
//! Molt runtime:    Calls encode/decode functions from this crate
//!                            │
//! buffa:           Pure-Rust protobuf wire format codec
//! ```

// Re-export core buffa types for downstream use.
pub use buffa::encoding::{self, Tag, WireType as BufWireType};
pub use buffa::error::{DecodeError, EncodeError};
pub use buffa::message::Message;
pub use buffa::types;
pub use buffa::view::MessageView;

/// Schema describing a protobuf message type for runtime encode/decode.
///
/// This will be populated by the Molt frontend from `.proto` file analysis
/// or from `@molt.proto` decorator metadata.
#[derive(Debug, Clone)]
pub struct MessageSchema {
    /// Fully qualified protobuf message name (e.g., "mypackage.MyMessage").
    pub name: String,
    /// Field definitions in field-number order.
    pub fields: Vec<FieldDef>,
}

/// A single field in a protobuf message schema.
#[derive(Debug, Clone)]
pub struct FieldDef {
    /// Protobuf field number (1-based).
    pub number: u32,
    /// Field name as it appears in Python.
    pub name: String,
    /// Wire type for encoding.
    pub wire_type: WireType,
    /// Whether this field is repeated.
    pub repeated: bool,
    /// Whether this field is optional (has presence tracking).
    pub optional: bool,
}

/// Protobuf wire types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WireType {
    /// Varint (int32, int64, uint32, uint64, sint32, sint64, bool, enum).
    Varint,
    /// 64-bit (fixed64, sfixed64, double).
    Fixed64,
    /// Length-delimited (string, bytes, embedded messages, packed repeated).
    LengthDelimited,
    /// 32-bit (fixed32, sfixed32, float).
    Fixed32,
}

impl WireType {
    /// Convert to buffa's wire type representation.
    pub fn to_buf(self) -> BufWireType {
        match self {
            WireType::Varint => BufWireType::Varint,
            WireType::Fixed64 => BufWireType::Fixed64,
            WireType::LengthDelimited => BufWireType::LengthDelimited,
            WireType::Fixed32 => BufWireType::Fixed32,
        }
    }
}

// ---------------------------------------------------------------------------
// Convenience wrappers around buffa's encoding primitives
// ---------------------------------------------------------------------------

/// Encode a u64 as a protobuf varint, appending to `buf`.
pub fn encode_varint(value: u64, buf: &mut Vec<u8>) {
    encoding::encode_varint(value, buf);
}

/// Decode a protobuf varint from the front of `data`.
/// Returns `(value, bytes_consumed)`.
pub fn decode_varint(data: &[u8]) -> Result<(u64, usize), DecodeError> {
    let len_before = data.len();
    let mut cursor = data;
    let value = encoding::decode_varint(&mut cursor)?;
    let consumed = len_before - cursor.len();
    Ok((value, consumed))
}

/// Encode a protobuf field tag (field number + wire type) and append to `buf`.
pub fn encode_tag(field_number: u32, wire_type: WireType, buf: &mut Vec<u8>) {
    Tag::new(field_number, wire_type.to_buf()).encode(buf);
}

/// Encode a complete varint field (tag + value) and append to `buf`.
pub fn encode_uint64_field(field_number: u32, value: u64, buf: &mut Vec<u8>) {
    encode_tag(field_number, WireType::Varint, buf);
    encode_varint(value, buf);
}

/// Encode a length-delimited field (tag + length + payload) and append to `buf`.
pub fn encode_bytes_field(field_number: u32, payload: &[u8], buf: &mut Vec<u8>) {
    encode_tag(field_number, WireType::LengthDelimited, buf);
    encode_varint(payload.len() as u64, buf);
    buf.extend_from_slice(payload);
}

/// Encode a string field (tag + length + UTF-8 bytes) and append to `buf`.
pub fn encode_string_field(field_number: u32, value: &str, buf: &mut Vec<u8>) {
    encode_bytes_field(field_number, value.as_bytes(), buf);
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn message_schema_construction() {
        let schema = MessageSchema {
            name: "test.MyMessage".into(),
            fields: vec![
                FieldDef {
                    number: 1,
                    name: "name".into(),
                    wire_type: WireType::LengthDelimited,
                    repeated: false,
                    optional: false,
                },
                FieldDef {
                    number: 2,
                    name: "value".into(),
                    wire_type: WireType::Varint,
                    repeated: false,
                    optional: false,
                },
            ],
        };
        assert_eq!(schema.name, "test.MyMessage");
        assert_eq!(schema.fields.len(), 2);
        assert_eq!(schema.fields[0].wire_type, WireType::LengthDelimited);
    }

    #[test]
    fn wire_type_equality() {
        assert_eq!(WireType::Varint, WireType::Varint);
        assert_ne!(WireType::Varint, WireType::Fixed64);
    }

    // -- varint roundtrip tests --

    #[test]
    fn varint_roundtrip_zero() {
        let mut buf = Vec::new();
        encode_varint(0, &mut buf);
        assert_eq!(buf, [0]);
        let (val, len) = decode_varint(&buf).unwrap();
        assert_eq!(val, 0);
        assert_eq!(len, 1);
    }

    #[test]
    fn varint_roundtrip_small() {
        let mut buf = Vec::new();
        encode_varint(150, &mut buf);
        assert_eq!(buf, [0x96, 0x01]);
        let (val, len) = decode_varint(&buf).unwrap();
        assert_eq!(val, 150);
        assert_eq!(len, 2);
    }

    #[test]
    fn varint_roundtrip_large() {
        let mut buf = Vec::new();
        encode_varint(u64::MAX, &mut buf);
        let (val, len) = decode_varint(&buf).unwrap();
        assert_eq!(val, u64::MAX);
        assert_eq!(len, 10); // max varint length
    }

    #[test]
    fn varint_roundtrip_powers_of_two() {
        for shift in 0..64u32 {
            let value = 1u64 << shift;
            let mut buf = Vec::new();
            encode_varint(value, &mut buf);
            let (decoded, _) = decode_varint(&buf).unwrap();
            assert_eq!(decoded, value, "failed at 2^{shift}");
        }
    }

    // -- field encoding tests --

    #[test]
    fn encode_uint64_field_roundtrip() {
        let mut buf = Vec::new();
        encode_uint64_field(1, 42, &mut buf);
        // Field 1, wire type 0 (varint) => tag byte = (1 << 3) | 0 = 0x08
        assert_eq!(buf[0], 0x08);
        // Value 42 fits in one byte
        assert_eq!(buf[1], 42);
        assert_eq!(buf.len(), 2);
    }

    #[test]
    fn encode_string_field_structure() {
        let mut buf = Vec::new();
        encode_string_field(2, "hello", &mut buf);
        // Field 2, wire type 2 (length-delimited) => tag byte = (2 << 3) | 2 = 0x12
        assert_eq!(buf[0], 0x12);
        // Length = 5
        assert_eq!(buf[1], 5);
        // Payload
        assert_eq!(&buf[2..], b"hello");
    }

    #[test]
    fn encode_bytes_field_empty() {
        let mut buf = Vec::new();
        encode_bytes_field(3, b"", &mut buf);
        // Field 3, wire type 2 => tag = (3 << 3) | 2 = 0x1A
        assert_eq!(buf[0], 0x1A);
        // Length = 0
        assert_eq!(buf[1], 0);
        assert_eq!(buf.len(), 2);
    }

    #[test]
    fn wire_type_to_buf_mapping() {
        // Verify our WireType maps correctly to buffa's WireType.
        assert_eq!(WireType::Varint.to_buf(), BufWireType::Varint);
        assert_eq!(WireType::Fixed64.to_buf(), BufWireType::Fixed64);
        assert_eq!(
            WireType::LengthDelimited.to_buf(),
            BufWireType::LengthDelimited
        );
        assert_eq!(WireType::Fixed32.to_buf(), BufWireType::Fixed32);
    }
}
