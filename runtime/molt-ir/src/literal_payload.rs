//! Canonical scalar and owned-literal admission and SimpleIR/TIR projection.
//!
//! Python strings may contain surrogate code points, whose surrogatepass UTF-8
//! bytes are not Rust `str`. Never decode, replace, or infer an absent payload.

use crate::OpIR;
use crate::tir::op_kinds_generated::{
    LiteralPayloadKind, OwnedLiteralPayloadKind, kind_to_opcode_table, opcode_canonical_kind_table,
    opcode_literal_payload_kind_table, opcode_owned_literal_payload_kind_table,
};
use crate::tir::ops::{AttrValue, TirOp};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LiteralPayload<'a> {
    Text(&'a str),
    Bytes(&'a [u8]),
}

impl<'a> LiteralPayload<'a> {
    fn from_parts(
        kind: crate::OpKind,
        shape: Option<OwnedLiteralPayloadKind>,
        text: Option<&'a str>,
        bytes: Option<&'a [u8]>,
    ) -> Result<Self, String> {
        match shape {
            Some(OwnedLiteralPayloadKind::String) => match (text, bytes) {
                (Some(text), Some(bytes)) if text.as_bytes() != bytes => {
                    Err("const_str has conflicting bytes and string payloads".into())
                }
                (_, Some(bytes)) => {
                    crate::python_string::PythonString::validate_utf8_surrogatepass(bytes)
                        .map_err(|offset| {
                            format!("const_str has invalid surrogatepass UTF-8 at byte {offset}")
                        })?;
                    Ok(Self::Bytes(bytes))
                }
                (Some(text), None) => Ok(Self::Text(text)),
                (None, None) => Err("const_str missing bytes or string payload".into()),
            },
            Some(OwnedLiteralPayloadKind::Bytes) => match (text, bytes) {
                (None, Some(bytes)) => Ok(Self::Bytes(bytes)),
                _ => Err("const_bytes requires bytes payload and forbids s_value".into()),
            },
            Some(OwnedLiteralPayloadKind::BigintBytes) => match (text, bytes) {
                (None, Some(bytes)) if !bytes.is_empty() => {
                    if bytes.len() > 1
                        && ((bytes[0] == 0 && bytes[1] < 0x80)
                            || (bytes[0] == 0xff && bytes[1] >= 0x80))
                    {
                        return Err("const_bigint has redundant sign extension".into());
                    }
                    Ok(Self::Bytes(bytes))
                }
                _ => Err("const_bigint requires nonempty signed bytes and forbids s_value".into()),
            },
            None => Err(format!("{kind} has no owned literal payload")),
        }
    }

    pub fn from_simple(op: &'a OpIR) -> Result<Self, String> {
        let shape = kind_to_opcode_table(op.kind).and_then(opcode_owned_literal_payload_kind_table);
        Self::from_parts(op.kind, shape, op.s_value.as_deref(), op.bytes.as_deref()).map_err(
            |error| {
                format!(
                    "{error} for output `{}`",
                    op.out.as_deref().unwrap_or("<missing>")
                )
            },
        )
    }

    pub fn from_tir(op: &'a TirOp) -> Result<Self, String> {
        let kind = opcode_canonical_kind_table(op.opcode)
            .ok_or_else(|| format!("{:?} has no wire literal payload", op.opcode))?;
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
        Self::from_parts(
            kind,
            opcode_owned_literal_payload_kind_table(op.opcode),
            text,
            bytes,
        )
    }

    pub fn as_bytes(self) -> &'a [u8] {
        match self {
            Self::Text(text) => text.as_bytes(),
            Self::Bytes(bytes) => bytes,
        }
    }
}

/// A validated literal value. Opcode membership and aliases come from the
/// generated payload shape; consumers match this value rather than opcodes.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SimpleLiteral<'a> {
    Int(i64),
    Float(f64),
    Bool(bool),
    None,
    Owned(OwnedLiteralPayloadKind, LiteralPayload<'a>),
}

impl<'a> SimpleLiteral<'a> {
    pub fn from_simple(op: &'a OpIR) -> Result<Option<Self>, String> {
        let Some(shape) = kind_to_opcode_table(op.kind).and_then(opcode_literal_payload_kind_table)
        else {
            return Ok(None);
        };
        let literal = match shape {
            LiteralPayloadKind::Int => Self::Int(
                op.value
                    .ok_or_else(|| format!("{} requires integer value payload", op.kind))?,
            ),
            LiteralPayloadKind::Float => Self::Float(
                op.f_value
                    .ok_or_else(|| format!("{} requires float f_value payload", op.kind))?,
            ),
            LiteralPayloadKind::Bool => Self::Bool(
                op.value
                    .ok_or_else(|| format!("{} requires bool value payload", op.kind))?
                    != 0,
            ),
            LiteralPayloadKind::None => Self::None,
            LiteralPayloadKind::Owned(kind) => Self::Owned(kind, LiteralPayload::from_simple(op)?),
        };
        Ok(Some(literal))
    }

    /// Exact bounded integer projection shared by all target admission callers.
    /// Boolean payloads remain a separate Python value domain.
    pub fn exact_integer_value(self, max_magnitude: u128) -> Option<i128> {
        let value = match self {
            Self::Int(value) => i128::from(value),
            Self::Owned(OwnedLiteralPayloadKind::BigintBytes, LiteralPayload::Bytes(bytes)) => {
                if bytes.len() > 16 {
                    return None;
                }
                let mut signed = [if bytes[0] & 0x80 == 0 { 0 } else { 0xff }; 16];
                signed[16 - bytes.len()..].copy_from_slice(bytes);
                i128::from_be_bytes(signed)
            }
            Self::Float(_) | Self::Bool(_) | Self::None | Self::Owned(_, _) => return None,
        };
        (value.unsigned_abs() <= max_magnitude).then_some(value)
    }
}

/// Canonical signed-byte transport for bounded backend constants and anchors.
/// Arbitrary-width frontend literals use the same minimal two's-complement
/// representation; this projection never formats a decimal string.
pub fn integer_literal_bytes(value: i128) -> Vec<u8> {
    let bytes = value.to_be_bytes();
    let mut start = 0;
    while start + 1 < bytes.len()
        && ((bytes[start] == 0 && bytes[start + 1] < 0x80)
            || (bytes[start] == 0xff && bytes[start + 1] >= 0x80))
    {
        start += 1;
    }
    bytes[start..].to_vec()
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
    let kind = opcode_canonical_kind_table(op.opcode)
        .ok_or_else(|| format!("{:?} has no wire literal representation", op.opcode))?;
    let mut simple = OpIR {
        out,
        ..crate::OpIR::new(kind)
    };
    match payload {
        LiteralPayload::Text(text) => simple.s_value = Some(text.to_owned()),
        LiteralPayload::Bytes(bytes) => simple.bytes = Some(bytes.to_vec()),
    }
    Ok(simple)
}

pub fn validate_simple_literal(op: &OpIR) -> Result<(), String> {
    SimpleLiteral::from_simple(op).map(|_| ())
}

pub fn validate_tir_literal(op: &TirOp) -> Result<(), String> {
    match opcode_literal_payload_kind_table(op.opcode) {
        Some(LiteralPayloadKind::Int) => match op.attrs.get("value") {
            Some(AttrValue::Int(_)) => Ok(()),
            _ => Err("integer literal requires Int attr value".into()),
        },
        Some(LiteralPayloadKind::Float) => match op.attrs.get("f_value") {
            Some(AttrValue::Float(_)) => Ok(()),
            _ => Err("float literal requires Float attr f_value".into()),
        },
        Some(LiteralPayloadKind::Bool) => match op.attrs.get("value") {
            Some(AttrValue::Bool(_) | AttrValue::Int(_)) => Ok(()),
            _ => Err("bool literal requires Bool or Int attr value".into()),
        },
        Some(LiteralPayloadKind::None) | None => Ok(()),
        Some(LiteralPayloadKind::Owned(_)) => LiteralPayload::from_tir(op).map(|_| ()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::ops::{AttrDict, Dialect, OpCode};
    use crate::tir::values::ValueId;

    fn tir(opcode: OpCode, attrs: AttrDict) -> TirOp {
        TirOp {
            origin: None,
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
            (OpCode::ConstStr, crate::OpKind::ConstStr, vec![]),
            (OpCode::ConstStr, crate::OpKind::ConstStr, vec![0, b'a', 0]),
            (
                OpCode::ConstStr,
                crate::OpKind::ConstStr,
                vec![0xed, 0xa0, 0x80],
            ),
            (
                OpCode::ConstStr,
                crate::OpKind::ConstStr,
                vec![0xed, 0xbf, 0xbf, 0],
            ),
            (OpCode::ConstBytes, crate::OpKind::ConstBytes, vec![]),
            (
                OpCode::ConstBytes,
                crate::OpKind::ConstBytes,
                vec![0, 0xff, 0x80],
            ),
            (OpCode::ConstBigInt, crate::OpKind::ConstBigint, vec![0]),
            (
                OpCode::ConstBigInt,
                crate::OpKind::ConstBigint,
                vec![0xff, 0x7f, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff],
            ),
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
        for (opcode, text) in [(OpCode::ConstStr, ""), (OpCode::ConstStr, "a\0é")] {
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
        for opcode in [OpCode::ScfFor, OpCode::ScfWhile] {
            let typed = tir(opcode, AttrDict::new());
            let error = lower_tir_literal(&typed, None).unwrap_err();
            assert!(error.contains("no wire literal payload"), "{error}");
        }
        for opcode in [OpCode::ConstStr, OpCode::ConstBytes, OpCode::ConstBigInt] {
            let typed = tir(opcode, AttrDict::new());
            assert!(validate_tir_literal(&typed).is_err());
            assert!(lower_tir_literal(&typed, None).is_err());
            let simple = OpIR::new(opcode_canonical_kind_table(opcode).unwrap());
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

    #[test]
    fn scalar_payload_admission_and_exact_integer_aliases_share_one_authority() {
        for kind in [
            crate::OpKind::Const,
            crate::OpKind::ConstInt,
            crate::OpKind::LoadConst,
            crate::OpKind::ConstBool,
            crate::OpKind::ConstFloat,
        ] {
            let op = OpIR {
                kind,
                ..crate::OpIR::new(kind)
            };
            assert!(validate_simple_literal(&op).is_err(), "{kind}");
            assert!(
                crate::ir_schema::validate_required_fields(&op).is_err(),
                "{kind}"
            );
        }
        for kind in [
            crate::OpKind::Const,
            crate::OpKind::ConstInt,
            crate::OpKind::LoadConst,
        ] {
            let op = OpIR {
                kind,
                value: Some(-7),
                ..crate::OpIR::new(kind)
            };
            let value = SimpleLiteral::from_simple(&op).unwrap().unwrap();
            assert_eq!(value.exact_integer_value(7), Some(-7));
            assert_eq!(value.exact_integer_value(6), None);
        }
        // Retired decimal transport is rejected, including otherwise valid text.
        for text in ["", "-", "+", " 1", "1 ", "1.0", "0xff", "１２", "123"] {
            let op = OpIR {
                kind: crate::OpKind::ConstBigint,
                s_value: Some(text.into()),
                ..crate::OpIR::new(crate::OpKind::ConstBigint)
            };
            assert!(validate_simple_literal(&op).is_err(), "{text:?}");
        }
        let huge = OpIR {
            kind: crate::OpKind::ConstBigint,
            bytes: Some([vec![1], vec![0; 32]].concat()),
            ..crate::OpIR::new(crate::OpKind::ConstBigint)
        };
        assert!(validate_simple_literal(&huge).is_ok());
        assert_eq!(
            SimpleLiteral::from_simple(&huge)
                .unwrap()
                .unwrap()
                .exact_integer_value(u128::MAX),
            None
        );
        for (opcode, key, attr) in [
            (OpCode::ConstInt, "value", AttrValue::Int(1)),
            (OpCode::ConstFloat, "f_value", AttrValue::Float(-0.0)),
            (OpCode::ConstBool, "value", AttrValue::Bool(true)),
        ] {
            assert!(validate_tir_literal(&tir(opcode, AttrDict::new())).is_err());
            assert!(
                validate_tir_literal(&tir(opcode, AttrDict::from([(key.into(), attr)]))).is_ok()
            );
            assert!(
                validate_tir_literal(&tir(
                    opcode,
                    AttrDict::from([(key.into(), AttrValue::Str("bad".into()))])
                ))
                .is_err()
            );
        }
        assert!(validate_tir_literal(&tir(OpCode::ConstNone, AttrDict::new())).is_ok());
    }

    #[test]
    fn signed_literal_bytes_are_minimal_and_project_exact_boundaries() {
        for (value, bytes) in [
            (0, vec![0]),
            (127, vec![0x7f]),
            (128, vec![0, 0x80]),
            (-1, vec![0xff]),
            (-128, vec![0x80]),
            (-129, vec![0xff, 0x7f]),
        ] {
            assert_eq!(integer_literal_bytes(value), bytes);
        }
        for value in [i128::MIN, -129, -128, -1, 0, 127, 128, i128::MAX] {
            let op = OpIR {
                kind: crate::OpKind::ConstBigint,
                bytes: Some(integer_literal_bytes(value)),
                ..crate::OpIR::new(crate::OpKind::ConstBigint)
            };
            let literal = SimpleLiteral::from_simple(&op).unwrap().unwrap();
            assert_eq!(literal.exact_integer_value(u128::MAX), Some(value));
        }
        for bytes in [
            vec![],
            vec![0, 0],
            vec![0, 127],
            vec![0xff, 0xff],
            vec![0xff, 0x80],
        ] {
            let op = OpIR {
                kind: crate::OpKind::ConstBigint,
                bytes: Some(bytes),
                ..crate::OpIR::new(crate::OpKind::ConstBigint)
            };
            assert!(validate_simple_literal(&op).is_err());
        }
    }
}
