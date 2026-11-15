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

use std::collections::HashMap;

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
    let mut seen: HashMap<ClassId, &str> = HashMap::new();
    for function in functions {
        for op in &function.ops {
            let Some(symbol) = op.class_definition.as_deref() else {
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
