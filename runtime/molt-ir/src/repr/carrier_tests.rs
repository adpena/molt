use super::*;

#[test]
fn scalar_annotations_never_raise_a_conservative_carrier() {
    for repr in [Repr::Never, Repr::MaybeBigInt, Repr::DynBox] {
        for annotation in [TirType::I64, TirType::F64, TirType::Bool, TirType::Never] {
            assert_eq!(repr.carrier_type(&annotation), TirType::DynBox);
        }
        for boxed in [
            TirType::Str,
            TirType::None,
            TirType::BigInt,
            TirType::DynBox,
        ] {
            assert_eq!(repr.carrier_type(&boxed), boxed);
        }
    }
}

#[test]
fn producer_proof_selects_every_raw_carrier_without_a_type_hint() {
    for (repr, machine) in [
        (Repr::RawI64Safe, TirType::I64),
        (Repr::RawI64FullDeopt, TirType::I64),
        (Repr::FloatUnboxed, TirType::F64),
        (Repr::Bool, TirType::Bool),
    ] {
        assert_eq!(repr.carrier_type(&TirType::DynBox), machine);
    }
}
