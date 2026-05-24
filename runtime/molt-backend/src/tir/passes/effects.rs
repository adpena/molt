//! Julia-style effects analysis for TIR functions and operations.
//!
//! Classifies known builtins and methods by their effects:
//! - `consistent`: pure function of inputs (same inputs -> same output)
//! - `effect_free`: no observable side effects
//! - `nothrow`: never raises exceptions (for valid inputs within domain)
//!
//! When all three properties hold, the function is PURE and eligible for
//! concrete evaluation (constant folding at compile time) when all arguments
//! are compile-time constants.
//!
//! The concrete evaluation logic lives in `sccp.rs`, which uses the effects
//! classification from this module to gate constant folding of calls.

use crate::tir::ops::{AttrValue, OpCode, TirOp};

pub(crate) const EFFECT_PROOF_ATTR: &str = "effect_proof";
pub(crate) const STATIC_MODULE_CLASS_BINDING_EFFECT_PROOF: &str = "static_module_class_binding";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum EffectProof {
    StaticModuleClassBinding,
}

impl EffectProof {
    #[inline]
    pub(crate) fn from_name(name: &str) -> Option<Self> {
        match name {
            STATIC_MODULE_CLASS_BINDING_EFFECT_PROOF => Some(Self::StaticModuleClassBinding),
            _ => None,
        }
    }

    #[inline]
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::StaticModuleClassBinding => STATIC_MODULE_CLASS_BINDING_EFFECT_PROOF,
        }
    }

    #[inline]
    pub(crate) fn is_valid_for_simple_ir_kind(self, kind: &str) -> bool {
        match self {
            Self::StaticModuleClassBinding => {
                matches!(kind, "module_cache_get" | "module_get_attr")
            }
        }
    }

    #[inline]
    fn is_valid_for_tir_opcode(self, opcode: OpCode) -> bool {
        match self {
            Self::StaticModuleClassBinding => {
                matches!(opcode, OpCode::ModuleCacheGet | OpCode::ModuleGetAttr)
            }
        }
    }
}

#[inline]
pub(crate) fn simple_ir_effect_proof(kind: &str, proof: Option<&str>) -> Option<EffectProof> {
    let proof = EffectProof::from_name(proof?)?;
    proof.is_valid_for_simple_ir_kind(kind).then_some(proof)
}

#[inline]
pub(crate) fn simple_ir_has_static_module_class_binding_effect_proof(
    kind: &str,
    proof: Option<&str>,
) -> bool {
    simple_ir_effect_proof(kind, proof) == Some(EffectProof::StaticModuleClassBinding)
}

#[inline]
pub(crate) fn tir_effect_proof(op: &TirOp) -> Option<EffectProof> {
    let proof_name = match op.attrs.get(EFFECT_PROOF_ATTR) {
        Some(AttrValue::Str(proof_name)) => proof_name,
        _ => return None,
    };
    let proof = EffectProof::from_name(proof_name)?;
    proof.is_valid_for_tir_opcode(op.opcode).then_some(proof)
}

#[inline]
pub(crate) fn tir_has_static_module_class_binding_effect_proof(op: &TirOp) -> bool {
    tir_effect_proof(op) == Some(EffectProof::StaticModuleClassBinding)
}

#[inline]
pub(super) fn opcode_may_throw(opcode: OpCode) -> bool {
    matches!(
        opcode,
        OpCode::Call
            | OpCode::CallMethod
            | OpCode::CallBuiltin
            | OpCode::Raise
            | OpCode::Index
            | OpCode::OrdAt
            | OpCode::StoreIndex
            | OpCode::LoadAttr
            | OpCode::StoreAttr
            | OpCode::DelAttr
            | OpCode::DelIndex
            | OpCode::Import
            | OpCode::ImportFrom
            | OpCode::ModuleCacheGet
            | OpCode::ModuleCacheSet
            | OpCode::ModuleCacheDel
            | OpCode::ModuleGetAttr
            | OpCode::ModuleGetGlobal
            | OpCode::ModuleGetName
            | OpCode::ModuleSetAttr
            | OpCode::ModuleDelGlobal
            | OpCode::ModuleDelGlobalIfPresent
            | OpCode::Div
            | OpCode::FloorDiv
            | OpCode::Mod
            | OpCode::GetIter
            | OpCode::IterNext
            | OpCode::IterNextUnboxed
            | OpCode::ForIter
            | OpCode::StateTransition
            | OpCode::ChanSendYield
            | OpCode::ChanRecvYield
            | OpCode::ClosureLoad
            | OpCode::ClosureStore
    )
}

#[inline]
pub(super) fn op_may_throw(op: &TirOp) -> bool {
    !tir_has_static_module_class_binding_effect_proof(op) && opcode_may_throw(op.opcode)
}

#[inline]
fn opcode_is_side_effecting(opcode: OpCode) -> bool {
    matches!(
        opcode,
        // Calls — may have arbitrary side effects.
        OpCode::Call
        | OpCode::CallMethod
        | OpCode::CallBuiltin
        // Store/delete mutations.
        | OpCode::StoreAttr
        | OpCode::StoreIndex
        | OpCode::DelAttr
        | OpCode::DelIndex
        // Control flow / exception handling.
        | OpCode::Raise
        | OpCode::CheckException
        | OpCode::TryStart
        | OpCode::TryEnd
        | OpCode::StateBlockStart
        | OpCode::StateBlockEnd
        | OpCode::StateSwitch
        | OpCode::StateTransition
        | OpCode::StateYield
        | OpCode::ChanSendYield
        | OpCode::ChanRecvYield
        | OpCode::ClosureStore
        // Generator protocol.
        | OpCode::Yield
        | OpCode::YieldFrom
        // Reference-counting and memory management.
        | OpCode::IncRef
        | OpCode::DecRef
        | OpCode::Free
        // Allocation may trigger a finalizer / GC hook.
        | OpCode::Alloc
        // Class-instance allocation: same finalizer-effect concern as
        // Alloc. ObjectNewBoundStack is intentionally NOT side-effecting:
        // escape-analysis only converts to it when the result provably does
        // not escape, and the lowered Cranelift StackSlot has no finalizer.
        | OpCode::ObjectNewBound
        // Import has module-level side effects.
        | OpCode::Import
        | OpCode::ImportFrom
        // Module lookup reads may raise on invalid names / missing attrs /
        // missing globals. Preserve even when the result is unused unless a
        // specific op instance carries a validated proof.
        | OpCode::ModuleCacheGet
        // Module mutations update runtime cache/module dictionaries and may
        // raise. Preserve even when their synthetic None result is unused.
        | OpCode::ModuleCacheSet
        | OpCode::ModuleCacheDel
        | OpCode::ModuleGetAttr
        | OpCode::ModuleGetGlobal
        | OpCode::ModuleGetName
        | OpCode::ModuleSetAttr
        | OpCode::ModuleDelGlobal
        | OpCode::ModuleDelGlobalIfPresent
        // IO / diagnostics — emits to stderr.
        | OpCode::WarnStderr
        // Deoptimisation must not be silently dropped.
        | OpCode::Deopt
    )
}

#[inline]
pub(super) fn op_is_side_effecting(op: &TirOp) -> bool {
    !tir_has_static_module_class_binding_effect_proof(op) && opcode_is_side_effecting(op.opcode)
}

#[inline]
pub(super) fn op_has_observable_effect_when_dead(op: &TirOp) -> bool {
    if op_is_side_effecting(op) || op_may_throw(op) {
        return true;
    }
    op.opcode == OpCode::Copy && op.attrs.contains_key("_original_kind")
}

/// Effect classification for a function or method.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FunctionEffects {
    /// Same inputs always produce the same output (referentially transparent).
    pub consistent: bool,
    /// No observable side effects (no I/O, no mutation of shared state).
    pub effect_free: bool,
    /// Never raises exceptions for valid inputs within the function's domain.
    pub nothrow: bool,
}

impl FunctionEffects {
    /// Returns `true` if the function is fully pure: consistent, effect-free,
    /// and nothrow. Only pure functions are eligible for concrete eval.
    #[inline]
    pub fn is_pure(&self) -> bool {
        self.consistent && self.effect_free && self.nothrow
    }
}

/// A fully pure function: consistent, effect-free, nothrow.
const PURE: FunctionEffects = FunctionEffects {
    consistent: true,
    effect_free: true,
    nothrow: true,
};

/// Look up effects for a known builtin function by name.
///
/// Returns `None` for unknown builtins -- the caller must treat unknown
/// functions as potentially having all effects (impure, side-effecting,
/// may-throw).
///
/// Only functions that are 100% pure for ALL valid inputs are classified
/// here. Functions that throw on invalid inputs (e.g. `int("abc")`) are
/// excluded because the nothrow property must hold unconditionally.
pub fn builtin_effects(name: &str) -> Option<FunctionEffects> {
    match name {
        // Type constructors (for literal/constant args only)
        "bool" | "int" | "float" => Some(PURE),

        // Numeric functions
        "abs" | "min" | "max" => Some(PURE),

        // Sequence functions (pure when operating on constant data)
        "len" | "sorted" | "reversed" | "sum" => Some(PURE),

        // String/repr (pure for constant primitive values)
        "str" | "repr" | "hash" => Some(PURE),

        // Character/encoding
        "chr" | "ord" | "hex" | "oct" | "bin" => Some(PURE),

        // Constructors that produce fresh immutable sequences
        "range" | "enumerate" | "zip" | "map" | "filter" | "tuple" | "frozenset" => Some(PURE),

        // Math module (pure numerical functions)
        "math.sqrt" | "math.floor" | "math.ceil" | "math.log" | "math.log2" | "math.log10"
        | "math.exp" | "math.sin" | "math.cos" | "math.tan" | "math.asin" | "math.acos"
        | "math.atan" | "math.atan2" | "math.fabs" | "math.pow" | "math.gcd" | "math.lcm"
        | "math.isfinite" | "math.isinf" | "math.isnan" | "math.copysign" | "math.trunc"
        | "math.hypot" => Some(PURE),

        // Explicitly NOT pure (I/O, random, time, mutation):
        // print, input, open, random.*, time.*, os.*, sys.*
        _ => None,
    }
}

/// Look up effects for a known method call on a type.
///
/// `receiver_type` is a hint about the receiver's type (e.g. "str", "list").
/// `method_name` is the method being called (e.g. "upper", "strip").
///
/// Only methods that are pure for all valid inputs are classified here.
/// Mutating methods (list.append, dict.update, etc.) are never pure.
pub fn method_effects(receiver_type: &str, method_name: &str) -> Option<FunctionEffects> {
    match (receiver_type, method_name) {
        // str methods -- strings are immutable, all these return new strings
        ("str", "upper")
        | ("str", "lower")
        | ("str", "strip")
        | ("str", "lstrip")
        | ("str", "rstrip")
        | ("str", "title")
        | ("str", "capitalize")
        | ("str", "casefold")
        | ("str", "swapcase")
        | ("str", "center")
        | ("str", "ljust")
        | ("str", "rjust")
        | ("str", "zfill")
        | ("str", "replace")
        | ("str", "join")
        | ("str", "split")
        | ("str", "rsplit")
        | ("str", "splitlines")
        | ("str", "startswith")
        | ("str", "endswith")
        | ("str", "find")
        | ("str", "rfind")
        | ("str", "index")
        | ("str", "rindex")
        | ("str", "count")
        | ("str", "isalpha")
        | ("str", "isdigit")
        | ("str", "isalnum")
        | ("str", "isspace")
        | ("str", "isupper")
        | ("str", "islower")
        | ("str", "istitle")
        | ("str", "isidentifier")
        | ("str", "isprintable")
        | ("str", "isdecimal")
        | ("str", "isnumeric")
        | ("str", "encode")
        | ("str", "expandtabs")
        | ("str", "removeprefix")
        | ("str", "removesuffix")
        | ("str", "partition")
        | ("str", "rpartition")
        | ("str", "maketrans")
        | ("str", "translate") => Some(PURE),

        // tuple methods -- tuples are immutable
        ("tuple", "count") | ("tuple", "index") => Some(PURE),

        // frozenset methods -- immutable
        ("frozenset", "union")
        | ("frozenset", "intersection")
        | ("frozenset", "difference")
        | ("frozenset", "symmetric_difference")
        | ("frozenset", "issubset")
        | ("frozenset", "issuperset")
        | ("frozenset", "isdisjoint")
        | ("frozenset", "copy") => Some(PURE),

        // int/float methods
        ("int", "bit_length")
        | ("int", "bit_count")
        | ("int", "to_bytes")
        | ("int", "conjugate")
        | ("float", "is_integer")
        | ("float", "hex")
        | ("float", "conjugate") => Some(PURE),

        // Explicitly NOT pure: list.append, list.extend, dict.update, set.add, etc.
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pure_builtins_are_pure() {
        for name in &[
            "len",
            "abs",
            "min",
            "max",
            "sum",
            "sorted",
            "bool",
            "int",
            "float",
            "str",
            "repr",
            "hash",
            "chr",
            "ord",
            "hex",
            "oct",
            "bin",
            "range",
            "enumerate",
            "zip",
            "math.sqrt",
            "math.floor",
            "math.ceil",
            "math.log",
        ] {
            let fx = builtin_effects(name).unwrap_or_else(|| panic!("{name} should have effects"));
            assert!(fx.is_pure(), "{name} should be pure");
        }
    }

    #[test]
    fn unknown_builtins_return_none() {
        assert!(builtin_effects("print").is_none());
        assert!(builtin_effects("input").is_none());
        assert!(builtin_effects("open").is_none());
        assert!(builtin_effects("random.random").is_none());
        assert!(builtin_effects("time.time").is_none());
    }

    #[test]
    fn str_methods_are_pure() {
        for method in &[
            "upper",
            "lower",
            "strip",
            "split",
            "replace",
            "startswith",
            "endswith",
            "find",
            "count",
            "join",
        ] {
            let fx = method_effects("str", method)
                .unwrap_or_else(|| panic!("str.{method} should have effects"));
            assert!(fx.is_pure(), "str.{method} should be pure");
        }
    }

    #[test]
    fn mutating_methods_are_unknown() {
        assert!(method_effects("list", "append").is_none());
        assert!(method_effects("list", "extend").is_none());
        assert!(method_effects("dict", "update").is_none());
        assert!(method_effects("set", "add").is_none());
    }
}
