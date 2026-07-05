//! Call-site fact model: confidence lattice, typed target facts, inline
//! eligibility, and the per-call-site fact record.
//!
//! The parent `call_facts` module owns construction, caching, and analysis.
//! This module owns only the portable data model consumed by those paths.

use crate::repr::Repr;

/// Identifier for a runtime guard that conditions a [`FactValue::Guarded`] fact
/// (for example, a class-version or type guard). Phase 3 populates these; Phase
/// 1 never emits `Guarded`, but the lattice variant exists so consumers can
/// fail closed on guarded facts before the guard machinery is live.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct GuardId(pub u32);

/// Observed-confidence weight for a [`FactValue::Profiled`] fact (0-255, scaled
/// from a profile observation). Phase 1 never emits `Profiled`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Confidence(pub u8);

/// Confidence lattice for one call-site fact.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FactValue {
    /// Statically established; no runtime check required.
    Proven,
    /// True under the named runtime guard. Phase 3.
    Guarded(GuardId),
    /// Observed at the given confidence; needs a guard to exploit soundly.
    /// Phase 3.
    Profiled(Confidence),
    /// Fail-closed default; assume the hazard holds.
    Unknown,
    /// Proven not to hold.
    False,
}

impl FactValue {
    /// True iff this is the statically-proven rung.
    #[inline]
    pub fn is_proven(self) -> bool {
        matches!(self, FactValue::Proven)
    }

    /// Encode a definitely-true or definitely-false static fact onto the
    /// lattice. A producer that merely lacks proof must emit
    /// [`FactValue::Unknown`] explicitly, not `False`.
    #[inline]
    pub fn from_decided(b: bool) -> FactValue {
        if b {
            FactValue::Proven
        } else {
            FactValue::False
        }
    }
}

/// The resolved target of a call site as a typed variant, never a decoded raw
/// marker bit.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CallTargetFact {
    /// A statically-resolved direct call to a function defined in this module.
    StaticDirect {
        /// The module-defined callee name.
        callee: String,
    },
    /// A dynamic, extern, runtime-helper, or otherwise unresolved target.
    Opaque,
}

impl CallTargetFact {
    /// The resolved callee name iff this is [`CallTargetFact::StaticDirect`].
    #[inline]
    pub fn static_callee(&self) -> Option<&str> {
        match self {
            CallTargetFact::StaticDirect { callee } => Some(callee.as_str()),
            CallTargetFact::Opaque => None,
        }
    }

    /// True iff the target is statically resolved.
    #[inline]
    pub fn is_static_direct(&self) -> bool {
        matches!(self, CallTargetFact::StaticDirect { .. })
    }
}

/// Why a callee is not eligible to inline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineWhyNot {
    Recursive,
    HasHandlers,
    Generator,
    EntryHasPredecessor,
    Closure,
    OverBudget,
}

/// Whether a callee may be inlined, and if not, the typed reason.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InlineEligibility {
    Eligible,
    WhyNot(InlineWhyNot),
    /// The target is not a statically-resolved module-defined callee.
    Unknown,
}

impl InlineEligibility {
    /// True iff the callee is eligible to inline.
    #[inline]
    pub fn is_eligible(self) -> bool {
        matches!(self, InlineEligibility::Eligible)
    }

    /// The typed why-not reason, if excluded for a concrete gate.
    #[inline]
    pub fn why_not(self) -> Option<InlineWhyNot> {
        match self {
            InlineEligibility::WhyNot(r) => Some(r),
            InlineEligibility::Eligible | InlineEligibility::Unknown => None,
        }
    }
}

/// The fact record attached to one call-bearing op.
#[derive(Debug, Clone, PartialEq)]
pub struct CallFacts {
    /// The typed call target. Never a raw marker bit.
    pub target: CallTargetFact,
    /// The result `Repr` when precise, else `None`.
    pub typed_return: Option<Repr>,
    /// The callee makes no further call of any kind.
    pub leaf: FactValue,
    /// The call provably cannot raise on this edge.
    pub no_throw: FactValue,
    /// The call performs no heap allocation. Phase 2 fills this.
    pub no_alloc: FactValue,
    /// Inline eligibility plus the typed why-not reason.
    pub inlinable: InlineEligibility,
}

impl CallFacts {
    /// The fully fail-closed record.
    pub fn unknown() -> CallFacts {
        CallFacts {
            target: CallTargetFact::Opaque,
            typed_return: None,
            leaf: FactValue::Unknown,
            no_throw: FactValue::Unknown,
            no_alloc: FactValue::Unknown,
            inlinable: InlineEligibility::Unknown,
        }
    }
}
