//! Typed class identity (HF-847 slice 1, design 78 section 5.1, D6).
//!
//! A [`ClassId`] names one class statement: the stable hash of the statement's
//! injective `definition_symbol` (`class:module:qualname[@line:col]`, HF-220),
//! computed once where an allocation's `class_definition` enters TIR. Two
//! classes that share a `__name__` in different modules have different ids;
//! an id compares as an integer and needs no table. Like rustc's
//! `DefPathHash`, the id does not depend on document order, so a cached TIR
//! body that holds one stays valid in any later document. Decode rejects two
//! class statements of one program whose ids collide ([`check_class_identities`]).

use crate::deterministic_hash::HashMap;

use sha2::{Digest, Sha256};

use crate::FunctionIR;

/// Identity of one class statement (I6: `TirType::Class(id)` is exact).
#[derive(
    Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, serde::Serialize, serde::Deserialize,
)]
#[serde(transparent)]
pub struct ClassId(pub u64);

impl ClassId {
    /// The id of the class statement `definition_symbol` names.
    pub fn of_definition(definition_symbol: &str) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"molt.class-id.v1\0");
        digest.update(definition_symbol.as_bytes());
        let bytes: [u8; 32] = digest.finalize().into();
        let mut word = [0u8; 8];
        word.copy_from_slice(&bytes[..8]);
        Self(u64::from_le_bytes(word))
    }
}

/// Reject a program in which two distinct class statements share an id.
///
/// Requires: `functions` is one whole document as decoded.
/// Raises: Err naming both statements on a collision.
pub fn check_class_identities(functions: &[FunctionIR]) -> Result<(), String> {
    let mut seen: HashMap<ClassId, &str> = HashMap::default();
    for function in functions {
        for op in &function.ops {
            let Some(symbol) = op
                .class_definition
                .as_deref()
                .or_else(|| op.exact_method.as_ref().map(|method| method.0.as_str()))
            else {
                continue;
            };
            let id = ClassId::of_definition(symbol);
            match seen.insert(id, symbol) {
                Some(other) if other != symbol => {
                    return Err(format!(
                        "class statements `{other}` and `{symbol}` share ClassId {:016x}",
                        id.0
                    ));
                }
                _ => {}
            }
        }
    }
    Ok(())
}

/// A positive proof that an allocation retains its class for its lifetime.
pub const STABLE_INSTANCE_CLASS_ATTR: &str = "stable_instance_class";

/// The class of a stable allocation, shared by SSA lift and type refinement.
pub fn stable_allocation_class(attrs: &crate::tir::ops::AttrDict) -> Option<ClassId> {
    use crate::tir::ops::AttrValue;
    match (
        attrs.get(STABLE_INSTANCE_CLASS_ATTR),
        attrs.get("class_definition"),
    ) {
        (Some(AttrValue::Bool(true)), Some(AttrValue::Str(symbol))) => {
            Some(ClassId::of_definition(symbol))
        }
        _ => None,
    }
}

/// A closed, ordinary method binding on the operation that creates its
/// function object. The operation's relocated callee and positional arity
/// identify the entry; the class statement and attribute name identify lookup.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ExactMethod(
    pub String,
    pub String,
    /// The sole run-once maker in a cell-eligible module. A later proven
    /// call can request a cell for this object from the owning document.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub Option<String>,
);

impl ExactMethod {
    pub const ATTR: &'static str = "exact_method";

    pub fn validate(&self, kind: Option<crate::OpKind>, arity: Option<i64>) -> Result<(), String> {
        if !matches!(
            kind,
            Some(crate::OpKind::FuncNew | crate::OpKind::FuncNewClosure)
        ) || self.0.is_empty()
            || self.1.is_empty()
            || self.2.as_deref().is_some_and(str::is_empty)
            || arity.is_none_or(|n| n < 1)
        {
            return Err("exact_method requires a class statement, member name and a function creation with positive positional arity".into());
        }
        Ok(())
    }

    pub fn encode(&self) -> crate::tir::ops::AttrValue {
        crate::tir::ops::AttrValue::Bytes(
            rmp_serde::to_vec(self).expect("method strings serialize"),
        )
    }

    pub fn from_tir(op: &crate::tir::ops::TirOp) -> Result<Option<Self>, String> {
        use crate::tir::ops::AttrValue;
        let Some(attr) = op.attrs.get(Self::ATTR) else {
            return Ok(None);
        };
        let AttrValue::Bytes(bytes) = attr else {
            return Err("invalid exact_method payload".into());
        };
        if op.opcode != crate::tir::ops::OpCode::Copy
            || op.attrs.contains_key("task_kind")
            || op.results.len() != 1
        {
            return Err("exact_method must describe a synchronous function creation".into());
        }
        let method: Self = rmp_serde::from_slice(bytes).map_err(|err| err.to_string())?;
        let arity = match op.attrs.get("value") {
            Some(AttrValue::Int(n)) => Some(*n),
            _ => None,
        };
        method.validate(op.origin, arity)?;
        Ok(Some(method))
    }
}

/// The TIR attribute that carries a class definition stamp's exact field
/// slots (`OpIR::exact_field_slots`).
pub const EXACT_FIELD_SLOTS_ATTR: &str = "exact_field_slots";

/// The TIR attribute that carries an allocation's proven instance plan, the
/// field word count (`OpIR::release_plan`).
pub const RELEASE_PLAN_ATTR: &str = "release_plan";

/// Encode `(name, offset)` field slots as the bytes of one TIR attribute:
/// per slot, the name's length (u32 little endian), its UTF-8 bytes, then the
/// offset (i64 little endian). [`decode_field_slots`] is the only reader.
pub fn encode_field_slots(slots: &[(String, i64)]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for (name, offset) in slots {
        let length = u32::try_from(name.len()).expect("a field name fits u32");
        bytes.extend_from_slice(&length.to_le_bytes());
        bytes.extend_from_slice(name.as_bytes());
        bytes.extend_from_slice(&offset.to_le_bytes());
    }
    bytes
}

/// The field slots [`encode_field_slots`] wrote, or `None` for bytes it did
/// not write.
pub fn decode_field_slots(mut bytes: &[u8]) -> Option<Vec<(String, i64)>> {
    let mut slots = Vec::new();
    while !bytes.is_empty() {
        let length = u32::from_le_bytes(bytes.get(..4)?.try_into().ok()?) as usize;
        bytes = &bytes[4..];
        let name = std::str::from_utf8(bytes.get(..length)?).ok()?.to_owned();
        bytes = &bytes[length..];
        let offset = i64::from_le_bytes(bytes.get(..8)?.try_into().ok()?);
        bytes = &bytes[8..];
        slots.push((name, offset));
    }
    Some(slots)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn same_named_classes_in_two_modules_have_distinct_ids() {
        let a = ClassId::of_definition("class:geometry:Point");
        let b = ClassId::of_definition("class:physics:Point");
        assert_ne!(a, b);
        assert_eq!(a, ClassId::of_definition("class:geometry:Point"));
    }
}
