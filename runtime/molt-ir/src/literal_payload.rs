//! Lossless owned-literal payload admission and SimpleIR/TIR projection.
//!
//! Python strings may contain surrogate code points, whose surrogatepass UTF-8
//! bytes are not Rust `str`. Never decode, replace, or infer an absent payload.

use crate::OpIR;
use crate::tir::op_kinds_generated::{kind_to_opcode_table, opcode_canonical_kind_table};
use crate::tir::ops::{AttrValue, OpCode, TirOp};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiteralPayload<'a> {
    Text(&'a str),
    Bytes(&'a [u8]),
}

fn owns_literal_payload(opcode: OpCode) -> bool {
    matches!(
        opcode,
        OpCode::ConstStr | OpCode::ConstBytes | OpCode::ConstBigInt
    )
}

impl<'a> LiteralPayload<'a> {
    fn from_parts(
        kind: &str,
        text: Option<&'a str>,
        bytes: Option<&'a [u8]>,
    ) -> Result<Self, String> {
        match kind {
            "const_str" => match (text, bytes) {
                (Some(text), Some(bytes)) if text.as_bytes() != bytes => {
                    Err("const_str has conflicting bytes and string payloads".into())
                }
                (_, Some(bytes)) => Ok(Self::Bytes(bytes)),
                (Some(text), None) => Ok(Self::Text(text)),
                (None, None) => Err("const_str missing bytes or string payload".into()),
            },
            "const_bytes" => match (text, bytes) {
                (None, Some(bytes)) => Ok(Self::Bytes(bytes)),
                _ => Err("const_bytes requires bytes payload and forbids s_value".into()),
            },
            "const_bigint" => match (text, bytes) {
                (Some(text), None) => Ok(Self::Text(text)),
                _ => Err("const_bigint requires decimal s_value and forbids bytes".into()),
            },
            _ => Err(format!("{kind} has no owned literal payload")),
        }
    }

    pub fn from_simple(op: &'a OpIR) -> Result<Self, String> {
        Self::from_parts(&op.kind, op.s_value.as_deref(), op.bytes.as_deref()).map_err(|error| {
            format!(
                "{error} for output `{}`",
                op.out.as_deref().unwrap_or("<missing>")
            )
        })
    }

    pub fn from_tir(op: &'a TirOp) -> Result<Self, String> {
        let kind = opcode_canonical_kind_table(op.opcode);
        let text = match op.attrs.get("s_value") {
            None => None,
            Some(AttrValue::Str(text)) => Some(text.as_str()),
            Some(_) => return Err(format!("{kind} requires string attr s_value")),
        };
        let bytes = match op.attrs.get("bytes") {
            None => None,
            Some(AttrValue::Bytes(bytes)) => Some(bytes.as_slice()),
            Some(_) => return Err(format!("{kind} requires bytes attr bytes")),
        };
        Self::from_parts(kind, text, bytes)
    }

    pub fn as_bytes(self) -> &'a [u8] {
        match self {
            Self::Text(text) => text.as_bytes(),
            Self::Bytes(bytes) => bytes,
        }
    }
}

pub fn required_simple_literal_bytes(op: &OpIR) -> &[u8] {
    LiteralPayload::from_simple(op)
        .unwrap_or_else(|error| panic!("{error}"))
        .as_bytes()
}

pub fn required_tir_literal_bytes(op: &TirOp) -> &[u8] {
    LiteralPayload::from_tir(op)
        .unwrap_or_else(|error| panic!("{error}"))
        .as_bytes()
}

pub fn lower_tir_literal(op: &TirOp, out: Option<String>) -> Result<OpIR, String> {
    let payload = LiteralPayload::from_tir(op)?;
    let mut simple = OpIR {
        kind: opcode_canonical_kind_table(op.opcode).into(),
        out,
        ..OpIR::default()
    };
    match payload {
        LiteralPayload::Text(text) => simple.s_value = Some(text.to_owned()),
        LiteralPayload::Bytes(bytes) => simple.bytes = Some(bytes.to_vec()),
    }
    Ok(simple)
}

pub fn validate_simple_literal(op: &OpIR) -> Result<(), String> {
    if kind_to_opcode_table(&op.kind).is_some_and(owns_literal_payload) {
        LiteralPayload::from_simple(op)?;
    }
    Ok(())
}

pub fn validate_tir_literal(op: &TirOp) -> Result<(), String> {
    if owns_literal_payload(op.opcode) {
        LiteralPayload::from_tir(op)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::ops::{AttrDict, Dialect};
    use crate::tir::values::ValueId;

    fn tir(opcode: OpCode, attrs: AttrDict) -> TirOp {
        TirOp {
            dialect: Dialect::Molt,
            opcode,
            operands: vec![],
            results: vec![ValueId(0)],
            attrs,
            source_span: None,
        }
    }

    #[test]
    fn literal_payload_admits_empty_nul_surrogate_and_arbitrary_bytes_losslessly() {
        for (opcode, kind, bytes) in [
            (OpCode::ConstStr, "const_str", vec![]),
            (OpCode::ConstStr, "const_str", vec![0, b'a', 0]),
            (OpCode::ConstStr, "const_str", vec![0xed, 0xa0, 0x80]),
            (OpCode::ConstStr, "const_str", vec![0xed, 0xbf, 0xbf, 0]),
            (OpCode::ConstBytes, "const_bytes", vec![]),
            (OpCode::ConstBytes, "const_bytes", vec![0, 0xff, 0x80]),
        ] {
            let typed = tir(
                opcode,
                AttrDict::from([("bytes".into(), AttrValue::Bytes(bytes.clone()))]),
            );
            assert_eq!(required_tir_literal_bytes(&typed), bytes);
            let simple = lower_tir_literal(&typed, Some("literal".into())).unwrap();
            assert_eq!(simple.kind, kind);
            assert_eq!(simple.bytes.as_deref(), Some(bytes.as_slice()));
            assert!(simple.s_value.is_none());
            assert_eq!(required_simple_literal_bytes(&simple), bytes);
        }
        for (opcode, text) in [
            (OpCode::ConstStr, ""),
            (OpCode::ConstStr, "a\0é"),
            (OpCode::ConstBigInt, "-9223372036854775809"),
        ] {
            let typed = tir(
                opcode,
                AttrDict::from([("s_value".into(), AttrValue::Str(text.into()))]),
            );
            let simple = lower_tir_literal(&typed, None).unwrap();
            assert_eq!(simple.s_value.as_deref(), Some(text));
            assert_eq!(required_simple_literal_bytes(&simple), text.as_bytes());
        }
    }

    #[test]
    fn literal_payload_rejects_absence_wrong_carrier_and_conflicts() {
        for opcode in [OpCode::ConstStr, OpCode::ConstBytes, OpCode::ConstBigInt] {
            let typed = tir(opcode, AttrDict::new());
            assert!(validate_tir_literal(&typed).is_err());
            assert!(lower_tir_literal(&typed, None).is_err());
            let simple = OpIR {
                kind: opcode_canonical_kind_table(opcode).into(),
                ..OpIR::default()
            };
            assert!(validate_simple_literal(&simple).is_err());
            assert!(crate::ir_schema::validate_required_fields(&simple).is_err());
        }
        for attrs in [
            AttrDict::from([("bytes".into(), AttrValue::Str("bad".into()))]),
            AttrDict::from([("s_value".into(), AttrValue::Bytes(vec![]))]),
            AttrDict::from([("value".into(), AttrValue::Str("noncanonical".into()))]),
            AttrDict::from([
                ("s_value".into(), AttrValue::Str("other".into())),
                ("bytes".into(), AttrValue::Bytes(vec![0])),
            ]),
        ] {
            assert!(validate_tir_literal(&tir(OpCode::ConstStr, attrs)).is_err());
        }
    }
}
