use crate::tir::types::TirType;

/// Scalar lane derived from the backend-facing TIR/LIR contract.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ScalarKind {
    Int,
    Bool,
    Float,
    Str,
    NoneValue,
}

/// Container dispatch lane derived from the backend-facing TIR/LIR contract.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ContainerKind {
    List,
    Dict,
    Set,
    Tuple,
    Str,
}

/// Physical container storage proof derived from structural producers.
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub enum ContainerStorageKind {
    FlatListInt,
}

/// A proven physical storage layout for a container value.
#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct ContainerStorageFact {
    pub(crate) kind: ContainerStorageKind,
    pub(crate) elem_ty: TirType,
}

/// The representation lattice: the physical carrier axis orthogonal to
/// [`TirType`].
///
/// `TirType` answers "what Python type is this value?"; `Repr` answers "what
/// is the physical carrier, and which unbox or raw-machine operations are sound
/// on it?". The trusted-unbox truncation bug class lives entirely on this
/// second axis: an `int`-typed value may be physically a heap `BigInt`, and that
/// distinction is invisible to the semantic type alone.
///
/// `Never` is the bottom element and `DynBox` is the top element. Integer
/// carriers have a BigInt-safe chain: `RawI64Safe` joins with `MaybeBigInt` to
/// `MaybeBigInt`, so a control-flow merge never silently treats an unknown-size
/// Python integer as a bare machine word. Distinct scalar families join to
/// `DynBox`; semantic subtyping such as Python `bool` being an `int` is owned by
/// `TirType`, not by this physical-carrier lattice.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Repr {
    /// Unreachable value.
    Never,
    /// Bare `i64` register/variable proven safe for raw machine arithmetic.
    RawI64Safe,
    /// Exactly `0` or `1`.
    Bool,
    /// NaN-boxed Python int of unknown magnitude: either inline or heap BigInt.
    /// Raw machine arithmetic and trusted unboxing are illegal on this carrier.
    MaybeBigInt,
    /// Bare `f64` register. Float lane.
    FloatUnboxed,
    /// NaN-box with fully unknown tag. No raw op is sound.
    DynBox,
}

impl Repr {
    /// Conservative representation floor for a semantic type. Integer-like
    /// values start BigInt-safe and can only be raised to raw carriers by proof.
    pub(crate) fn default_for(ty: &TirType) -> Repr {
        match ty {
            TirType::I64 | TirType::BigInt => Repr::MaybeBigInt,
            TirType::Bool => Repr::Bool,
            TirType::F64 => Repr::FloatUnboxed,
            TirType::Never => Repr::Never,
            _ => Repr::DynBox,
        }
    }

    /// True when the carrier is a bare i64 and raw machine arithmetic is sound.
    pub fn is_raw_i64_safe(self) -> bool {
        matches!(self, Repr::RawI64Safe)
    }

    /// Least upper bound for carrier facts at control-flow joins.
    ///
    /// The operation is deliberately fail-closed: only the integer raw/boxed
    /// relationship has a non-top mixed join today. Bool and float carriers do
    /// not get silently coerced into integer carriers by this lattice.
    pub fn join(self, other: Repr) -> Repr {
        use Repr::*;

        match (self, other) {
            (a, b) if a == b => a,
            (Never, b) | (b, Never) => b,
            (DynBox, _) | (_, DynBox) => DynBox,
            (RawI64Safe, MaybeBigInt) | (MaybeBigInt, RawI64Safe) => MaybeBigInt,
            _ => DynBox,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_for_floors_int_to_maybe_bigint() {
        assert_eq!(Repr::default_for(&TirType::I64), Repr::MaybeBigInt);
        assert_eq!(Repr::default_for(&TirType::BigInt), Repr::MaybeBigInt);
        assert_eq!(Repr::default_for(&TirType::Bool), Repr::Bool);
        assert_eq!(Repr::default_for(&TirType::F64), Repr::FloatUnboxed);
        assert_eq!(Repr::default_for(&TirType::Never), Repr::Never);
        assert_eq!(Repr::default_for(&TirType::Str), Repr::DynBox);
        assert_eq!(Repr::default_for(&TirType::None), Repr::DynBox);
        assert_eq!(
            Repr::default_for(&TirType::List(Box::new(TirType::I64))),
            Repr::DynBox
        );
        assert_eq!(
            Repr::default_for(&TirType::UserClass("m.Point".into())),
            Repr::DynBox
        );
        assert_eq!(Repr::default_for(&TirType::DynBox), Repr::DynBox);

        for ty in [
            TirType::I64,
            TirType::BigInt,
            TirType::Str,
            TirType::None,
            TirType::DynBox,
            TirType::UserClass("m.C".into()),
        ] {
            assert!(
                !Repr::default_for(&ty).is_raw_i64_safe(),
                "type {ty:?} must not floor to a raw i64 carrier"
            );
        }
    }

    #[test]
    fn carrier_view_predicates() {
        assert!(Repr::RawI64Safe.is_raw_i64_safe());
        for repr in [
            Repr::MaybeBigInt,
            Repr::Bool,
            Repr::FloatUnboxed,
            Repr::DynBox,
            Repr::Never,
        ] {
            assert!(!repr.is_raw_i64_safe(), "{repr:?} is not raw-i64-safe");
        }
    }

    #[test]
    fn join_is_commutative_and_idempotent() {
        let reprs = [
            Repr::Never,
            Repr::RawI64Safe,
            Repr::Bool,
            Repr::MaybeBigInt,
            Repr::FloatUnboxed,
            Repr::DynBox,
        ];

        for lhs in reprs {
            assert_eq!(lhs.join(lhs), lhs);
            for rhs in reprs {
                assert_eq!(lhs.join(rhs), rhs.join(lhs));
            }
        }
    }

    #[test]
    fn join_respects_bottom_top_and_integer_floor() {
        assert_eq!(Repr::Never.join(Repr::RawI64Safe), Repr::RawI64Safe);
        assert_eq!(Repr::DynBox.join(Repr::RawI64Safe), Repr::DynBox);
        assert_eq!(Repr::RawI64Safe.join(Repr::MaybeBigInt), Repr::MaybeBigInt);
    }

    #[test]
    fn join_distinct_scalar_families_fail_closed_to_dynbox() {
        for pair in [
            (Repr::RawI64Safe, Repr::Bool),
            (Repr::RawI64Safe, Repr::FloatUnboxed),
            (Repr::MaybeBigInt, Repr::Bool),
            (Repr::MaybeBigInt, Repr::FloatUnboxed),
            (Repr::Bool, Repr::FloatUnboxed),
        ] {
            assert_eq!(pair.0.join(pair.1), Repr::DynBox, "{pair:?}");
        }
    }
}
