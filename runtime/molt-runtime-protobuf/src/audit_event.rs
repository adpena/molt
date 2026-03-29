//! AuditEvent protobuf schema and convenience encode/decode.
//!
//! Field numbers match the canonical AuditEvent.proto:
//!   1: timestamp_ns (uint64)
//!   2: operation (string)
//!   3: capability (string)
//!   4: decision (uint64, 0=Allowed, 1=Denied, 2=ResourceExceeded)
//!   5: module (string)

use crate::decode::{decode_message, MessageDecodeError};
use crate::encode::{encode_message, FieldValue};
use crate::{FieldDef, MessageSchema, WireType};

/// Returns the canonical `MessageSchema` for an `AuditEvent` message.
pub fn audit_event_schema() -> MessageSchema {
    MessageSchema {
        name: "molt.audit.AuditEvent".into(),
        fields: vec![
            FieldDef {
                number: 1,
                name: "timestamp_ns".into(),
                wire_type: WireType::Varint,
                repeated: false,
                optional: false,
            },
            FieldDef {
                number: 2,
                name: "operation".into(),
                wire_type: WireType::LengthDelimited,
                repeated: false,
                optional: false,
            },
            FieldDef {
                number: 3,
                name: "capability".into(),
                wire_type: WireType::LengthDelimited,
                repeated: false,
                optional: false,
            },
            FieldDef {
                number: 4,
                name: "decision".into(),
                wire_type: WireType::Varint,
                repeated: false,
                optional: false,
            },
            FieldDef {
                number: 5,
                name: "module".into(),
                wire_type: WireType::LengthDelimited,
                repeated: false,
                optional: false,
            },
        ],
    }
}

/// Decoded representation of an `AuditEvent` protobuf message.
#[derive(Debug, Clone, PartialEq)]
pub struct DecodedAuditEvent {
    /// Nanoseconds since Unix epoch.
    pub timestamp_ns: u64,
    /// Name of the operation being audited (e.g. `"fs.read"`).
    pub operation: String,
    /// Capability required for the operation (e.g. `"fs.read"`).
    pub capability: String,
    /// Decision code: 0 = Allowed, 1 = Denied, 2 = ResourceExceeded.
    pub decision: u64,
    /// Name of the Python module that triggered the event.
    pub module_name: String,
}

/// Encode an `AuditEvent` to protobuf wire format.
pub fn encode_audit_event(
    timestamp_ns: u64,
    operation: &str,
    capability: &str,
    decision: u64,
    module_name: &str,
) -> Vec<u8> {
    let schema = audit_event_schema();
    let values = vec![
        FieldValue::Uint64(timestamp_ns),
        FieldValue::Bytes(operation.as_bytes().to_vec()),
        FieldValue::Bytes(capability.as_bytes().to_vec()),
        FieldValue::Uint64(decision),
        FieldValue::Bytes(module_name.as_bytes().to_vec()),
    ];
    encode_message(&schema, &values)
        .expect("encode_audit_event: schema and values are always in sync")
}

/// Decode an `AuditEvent` from protobuf wire format.
pub fn decode_audit_event(data: &[u8]) -> Result<DecodedAuditEvent, MessageDecodeError> {
    let schema = audit_event_schema();
    let values = decode_message(&schema, data)?;

    // values is in schema field order: [timestamp_ns, operation, capability, decision, module]
    let timestamp_ns = match &values[0] {
        FieldValue::Uint64(v) => *v,
        _ => 0,
    };
    let operation = match &values[1] {
        FieldValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
        _ => String::new(),
    };
    let capability = match &values[2] {
        FieldValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
        _ => String::new(),
    };
    let decision = match &values[3] {
        FieldValue::Uint64(v) => *v,
        _ => 0,
    };
    let module_name = match &values[4] {
        FieldValue::Bytes(b) => String::from_utf8_lossy(b).into_owned(),
        _ => String::new(),
    };

    Ok(DecodedAuditEvent {
        timestamp_ns,
        operation,
        capability,
        decision,
        module_name,
    })
}
