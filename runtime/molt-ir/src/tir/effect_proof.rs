//! Validated effect-proof vocabulary shared by SimpleIR schema and TIR passes.
//!
//! This module is a vocabulary authority, not a pass. Keeping it outside
//! `tir::passes` lets SimpleIR transport validate proof names without depending
//! upward on pass implementations, which is required before the vocabulary can
//! move into `molt-ir`.

use crate::tir::ops::{AttrValue, OpCode, TirOp};

pub const EFFECT_PROOF_ATTR: &str = "effect_proof";
pub const STATIC_MODULE_CLASS_BINDING_EFFECT_PROOF: &str = "static_module_class_binding";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EffectProof {
    StaticModuleClassBinding,
}

impl EffectProof {
    #[inline]
    pub fn from_name(name: &str) -> Option<Self> {
        match name {
            STATIC_MODULE_CLASS_BINDING_EFFECT_PROOF => Some(Self::StaticModuleClassBinding),
            _ => None,
        }
    }

    #[inline]
    pub fn name(self) -> &'static str {
        match self {
            Self::StaticModuleClassBinding => STATIC_MODULE_CLASS_BINDING_EFFECT_PROOF,
        }
    }

    #[inline]
    pub fn is_valid_for_simple_ir_kind(self, kind: &str) -> bool {
        match self {
            Self::StaticModuleClassBinding => {
                matches!(kind, "module_cache_get" | "module_get_attr")
            }
        }
    }

    #[inline]
    pub fn is_valid_for_tir_opcode(self, opcode: OpCode) -> bool {
        match self {
            Self::StaticModuleClassBinding => {
                matches!(opcode, OpCode::ModuleCacheGet | OpCode::ModuleGetAttr)
            }
        }
    }
}

#[inline]
pub fn simple_ir_effect_proof(kind: &str, proof: Option<&str>) -> Option<EffectProof> {
    let proof = EffectProof::from_name(proof?)?;
    proof.is_valid_for_simple_ir_kind(kind).then_some(proof)
}

#[inline]
pub fn simple_ir_has_static_module_class_binding_effect_proof(
    kind: &str,
    proof: Option<&str>,
) -> bool {
    simple_ir_effect_proof(kind, proof) == Some(EffectProof::StaticModuleClassBinding)
}

#[inline]
pub fn tir_effect_proof(op: &TirOp) -> Option<EffectProof> {
    let proof_name = match op.attrs.get(EFFECT_PROOF_ATTR) {
        Some(AttrValue::Str(proof_name)) => proof_name,
        _ => return None,
    };
    let proof = EffectProof::from_name(proof_name)?;
    proof.is_valid_for_tir_opcode(op.opcode).then_some(proof)
}

#[inline]
pub fn tir_has_static_module_class_binding_effect_proof(op: &TirOp) -> bool {
    tir_effect_proof(op) == Some(EffectProof::StaticModuleClassBinding)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tir::ops::{Dialect, TirOp};

    #[test]
    fn static_module_class_binding_validates_same_simple_ir_and_tir_family() {
        let proof = STATIC_MODULE_CLASS_BINDING_EFFECT_PROOF;

        assert_eq!(
            simple_ir_effect_proof("module_cache_get", Some(proof)),
            Some(EffectProof::StaticModuleClassBinding)
        );
        assert_eq!(
            simple_ir_effect_proof("module_get_attr", Some(proof)),
            Some(EffectProof::StaticModuleClassBinding)
        );
        assert_eq!(simple_ir_effect_proof("call", Some(proof)), None);

        let op = TirOp {
            dialect: Dialect::Molt,
            opcode: OpCode::ModuleCacheGet,
            operands: Vec::new(),
            results: Vec::new(),
            attrs: [(
                EFFECT_PROOF_ATTR.to_string(),
                AttrValue::Str(proof.to_string()),
            )]
            .into_iter()
            .collect(),
            source_span: None,
        };
        assert!(tir_has_static_module_class_binding_effect_proof(&op));
    }
}
