//! Protobuf serialization for Molt-compiled Python programs.
//!
//! This crate bridges Molt's runtime with Anthropic's `buffa` protobuf
//! library, providing Python-accessible protobuf encoding and decoding
//! with zero-copy views for incoming data.
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
//! Molt runtime:    Calls buffa::Message::encode_to_vec() /
//!                  buffa::Message::decode_from_slice()
//!                            │
//! buffa:           Pure-Rust protobuf wire format codec
//! ```
//!
//! # Status
//!
//! This crate is a scaffold. The buffa dependency will be added when:
//! 1. The Python-to-protobuf type mapping is designed
//! 2. The `@molt.proto` decorator is implemented in the frontend
//! 3. Code generation for message types is integrated
//!
//! # Future API
//!
//! ```ignore
//! // Encode a Molt object to protobuf binary
//! pub fn encode_to_vec(obj_bits: u64, schema: &MessageSchema) -> Vec<u8>;
//!
//! // Decode protobuf binary to a Molt object (owned)
//! pub fn decode_from_slice(data: &[u8], schema: &MessageSchema) -> u64;
//!
//! // Decode protobuf binary to a zero-copy view (borrowed)
//! pub fn decode_view(data: &[u8], schema: &MessageSchema) -> u64;
//! ```

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
}
