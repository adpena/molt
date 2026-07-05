use crate::tir::op_kinds_generated::{
    GvnValueKeyKind, GvnValueKeySpec, opcode_gvn_value_key_spec_table,
};
use crate::tir::ops::{AttrValue, OpCode, TirOp};
use crate::tir::values::ValueId;

/// Exact literal payload identity for same-block constant value numbering.
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub(super) enum GvnValueKey {
    I64(i64),
    Bool(bool),
    NoneSingleton,
    F64Bits(u64),
    Str(String),
    Bytes(Vec<u8>),
}

/// A hashable representation of a computation for value numbering.
#[derive(Hash, Eq, PartialEq, Clone, Debug)]
pub(super) struct ValueKey {
    pub(super) opcode: OpCode,
    /// Operand value numbers (canonicalized through the scoped leader table).
    pub(super) operands: Vec<ValueId>,
    /// Optional attr payload participating in value identity. The payload shape
    /// is generated from op_kinds.toml; if a required attr is missing/malformed,
    /// the op is not numbered rather than colliding through a local default.
    pub(super) attr_key: Option<GvnValueKey>,
}

/// A type is "primitive" when arithmetic on it is provably side-effect-free.
pub(super) fn is_primitive_type(ty: &crate::tir::types::TirType) -> bool {
    use crate::tir::types::TirType;
    matches!(
        ty,
        TirType::I64 | TirType::F64 | TirType::Bool | TirType::None
    )
}

fn attr_for_gvn_value_key(op: &TirOp, spec: GvnValueKeySpec) -> Option<&AttrValue> {
    spec.attrs.iter().find_map(|attr| op.attrs.get(*attr))
}

pub(super) fn gvn_value_key_from_spec(op: &TirOp, spec: GvnValueKeySpec) -> Option<GvnValueKey> {
    match spec.kind {
        GvnValueKeyKind::I64Attr => match attr_for_gvn_value_key(op, spec) {
            Some(AttrValue::Int(i)) => Some(GvnValueKey::I64(*i)),
            _ => None,
        },
        GvnValueKeyKind::BoolAttr => match attr_for_gvn_value_key(op, spec) {
            Some(AttrValue::Bool(b)) => Some(GvnValueKey::Bool(*b)),
            Some(AttrValue::Int(i)) => Some(GvnValueKey::Bool(*i != 0)),
            _ => None,
        },
        GvnValueKeyKind::NoneSingleton => Some(GvnValueKey::NoneSingleton),
        GvnValueKeyKind::F64BitsAttr => match attr_for_gvn_value_key(op, spec) {
            Some(AttrValue::Float(f)) => Some(GvnValueKey::F64Bits(f.to_bits())),
            _ => None,
        },
        GvnValueKeyKind::StrAttr => match attr_for_gvn_value_key(op, spec) {
            Some(AttrValue::Str(s)) => Some(GvnValueKey::Str(s.clone())),
            _ => None,
        },
        GvnValueKeyKind::BytesAttr => match attr_for_gvn_value_key(op, spec) {
            Some(AttrValue::Bytes(b)) => Some(GvnValueKey::Bytes(b.clone())),
            _ => None,
        },
    }
}

/// Extract a generated GVN value-key attr payload.
pub(super) fn gvn_value_key(op: &TirOp) -> Option<GvnValueKey> {
    let spec = opcode_gvn_value_key_spec_table(op.opcode)?;
    gvn_value_key_from_spec(op, spec)
}
