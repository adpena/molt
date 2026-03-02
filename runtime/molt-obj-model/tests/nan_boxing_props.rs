//! Property-based tests for the NaN-boxed object model (`MoltObject`).
//!
//! These tests use proptest to verify round-trip invariants, tag uniqueness,
//! and cross-type non-collision guarantees for every NaN-boxing lane.

use proptest::prelude::*;

use molt_obj_model::MoltObject;

/// The inline integer range is 47-bit signed: [-(2^46), 2^46 - 1].
const INT_MIN_INLINE: i64 = -(1i64 << 46);
const INT_MAX_INLINE: i64 = (1i64 << 46) - 1;

/// The canonical NaN bit pattern produced by `MoltObject::from_float(NaN)`.
/// Matches the crate-internal `CANONICAL_NAN_BITS`.
const CANONICAL_NAN_BITS: u64 = 0x7ff0_0000_0000_0001;

proptest! {
    // ------------------------------------------------------------------
    // Float round-trip
    // ------------------------------------------------------------------

    /// Any f64 survives round-trip through from_float / as_float.
    /// NaN inputs are canonicalized to CANONICAL_NAN_BITS.
    #[test]
    fn float_roundtrip(f in any::<f64>()) {
        let v = MoltObject::from_float(f);
        if f.is_nan() {
            // All NaN variants must collapse to the single canonical NaN.
            prop_assert_eq!(v.bits(), CANONICAL_NAN_BITS);
            // The canonical NaN must still decode as a NaN float.
            // Note: is_float() is false for the canonical NaN because its
            // QNAN bits are set, but as_float() returns None. Instead we
            // verify the bit pattern directly.
            let decoded = f64::from_bits(v.bits());
            prop_assert!(decoded.is_nan());
        } else {
            prop_assert!(v.is_float());
            let out = v.as_float().unwrap();
            prop_assert_eq!(f.to_bits(), out.to_bits());
        }
    }

    // ------------------------------------------------------------------
    // Int round-trip
    // ------------------------------------------------------------------

    /// Any i64 in the inline range [INT_MIN_INLINE, INT_MAX_INLINE] survives
    /// from_int / as_int round-trip.
    #[test]
    fn int_roundtrip(i in INT_MIN_INLINE..=INT_MAX_INLINE) {
        let v = MoltObject::from_int(i);
        prop_assert!(v.is_int());
        prop_assert_eq!(v.as_int(), Some(i));
    }

    /// Negative integers survive round-trip correctly (sign-extension).
    #[test]
    fn negative_int_roundtrip(i in INT_MIN_INLINE..0i64) {
        let v = MoltObject::from_int(i);
        prop_assert!(v.is_int());
        prop_assert_eq!(v.as_int(), Some(i));
    }

    // ------------------------------------------------------------------
    // Bool round-trip
    // ------------------------------------------------------------------

    /// Bool values produce distinct, correct round-trips.
    #[test]
    fn bool_roundtrip(b in any::<bool>()) {
        let v = MoltObject::from_bool(b);
        prop_assert!(v.is_bool());
        prop_assert_eq!(v.as_bool(), Some(b));
    }

    // ------------------------------------------------------------------
    // None singleton
    // ------------------------------------------------------------------

    /// `MoltObject::none()` always produces the same bit pattern and
    /// correctly identifies as none.
    #[test]
    fn none_is_stable(_seed in 0u32..1000) {
        let v = MoltObject::none();
        prop_assert!(v.is_none());
        // Every invocation must produce an identical bit pattern.
        prop_assert_eq!(v.bits(), MoltObject::none().bits());
    }

    // ------------------------------------------------------------------
    // Cross-type non-collision: float vs. int
    // ------------------------------------------------------------------

    /// A non-NaN float must never be mistaken for an int, bool, none, or ptr.
    #[test]
    fn float_not_other_types(f in any::<f64>().prop_filter("not NaN", |f| !f.is_nan())) {
        let v = MoltObject::from_float(f);
        prop_assert!(v.is_float());
        prop_assert!(!v.is_int());
        prop_assert!(!v.is_bool());
        prop_assert!(!v.is_none());
        prop_assert!(!v.is_ptr());
        prop_assert!(!v.is_pending());
    }

    // ------------------------------------------------------------------
    // Cross-type non-collision: int vs. others
    // ------------------------------------------------------------------

    /// An int must never be mistaken for a float, bool, none, or ptr.
    #[test]
    fn int_not_other_types(i in INT_MIN_INLINE..=INT_MAX_INLINE) {
        let v = MoltObject::from_int(i);
        prop_assert!(v.is_int());
        prop_assert!(!v.is_float());
        prop_assert!(!v.is_bool());
        prop_assert!(!v.is_none());
        prop_assert!(!v.is_ptr());
        prop_assert!(!v.is_pending());
    }

    // ------------------------------------------------------------------
    // Cross-type non-collision: bool vs. others
    // ------------------------------------------------------------------

    /// A bool must never be mistaken for a float, int, none, or ptr.
    #[test]
    fn bool_not_other_types(b in any::<bool>()) {
        let v = MoltObject::from_bool(b);
        prop_assert!(v.is_bool());
        prop_assert!(!v.is_float());
        prop_assert!(!v.is_int());
        prop_assert!(!v.is_none());
        prop_assert!(!v.is_ptr());
        prop_assert!(!v.is_pending());
    }

    // ------------------------------------------------------------------
    // None vs. others
    // ------------------------------------------------------------------

    /// None must never be mistaken for a float, int, bool, or ptr.
    #[test]
    fn none_not_other_types(_seed in 0u32..1000) {
        let v = MoltObject::none();
        prop_assert!(v.is_none());
        prop_assert!(!v.is_float());
        prop_assert!(!v.is_int());
        prop_assert!(!v.is_bool());
        prop_assert!(!v.is_ptr());
        prop_assert!(!v.is_pending());
    }

    // ------------------------------------------------------------------
    // Pending vs. others
    // ------------------------------------------------------------------

    /// Pending must never be mistaken for a float, int, bool, none, or ptr.
    #[test]
    fn pending_not_other_types(_seed in 0u32..1000) {
        let v = MoltObject::pending();
        prop_assert!(v.is_pending());
        prop_assert!(!v.is_float());
        prop_assert!(!v.is_int());
        prop_assert!(!v.is_bool());
        prop_assert!(!v.is_none());
        prop_assert!(!v.is_ptr());
    }

    // ------------------------------------------------------------------
    // Bit-level invariants
    // ------------------------------------------------------------------

    /// The QNAN prefix bits are always set for non-float tagged types.
    #[test]
    fn tagged_types_have_qnan_prefix(i in INT_MIN_INLINE..=INT_MAX_INLINE) {
        let int_v = MoltObject::from_int(i);
        // Upper bits must include QNAN
        prop_assert_eq!(int_v.bits() & 0x7ff8_0000_0000_0000, 0x7ff8_0000_0000_0000);
    }

    /// `from_bits(v.bits())` is identity for any constructed MoltObject.
    #[test]
    fn from_bits_identity_float(f in any::<f64>()) {
        let v = MoltObject::from_float(f);
        let v2 = MoltObject::from_bits(v.bits());
        prop_assert_eq!(v, v2);
    }

    #[test]
    fn from_bits_identity_int(i in INT_MIN_INLINE..=INT_MAX_INLINE) {
        let v = MoltObject::from_int(i);
        let v2 = MoltObject::from_bits(v.bits());
        prop_assert_eq!(v, v2);
    }

    #[test]
    fn from_bits_identity_bool(b in any::<bool>()) {
        let v = MoltObject::from_bool(b);
        let v2 = MoltObject::from_bits(v.bits());
        prop_assert_eq!(v, v2);
    }

    // ------------------------------------------------------------------
    // Int boundary stress
    // ------------------------------------------------------------------

    /// The extreme boundary values of the inline int range must round-trip.
    #[test]
    fn int_boundary_values(i in prop::sample::select(vec![
        INT_MIN_INLINE,
        INT_MIN_INLINE + 1,
        -1i64,
        0i64,
        1i64,
        INT_MAX_INLINE - 1,
        INT_MAX_INLINE,
    ])) {
        let v = MoltObject::from_int(i);
        prop_assert!(v.is_int());
        prop_assert_eq!(v.as_int(), Some(i));
    }

    // ------------------------------------------------------------------
    // Float special values
    // ------------------------------------------------------------------

    /// Special float values (infinities, zeros, subnormals) must round-trip.
    #[test]
    fn float_special_values(f in prop::sample::select(vec![
        0.0f64,
        -0.0f64,
        f64::INFINITY,
        f64::NEG_INFINITY,
        f64::MIN,
        f64::MAX,
        f64::MIN_POSITIVE,         // smallest positive normal
        5e-324f64,                  // smallest positive subnormal
    ])) {
        let v = MoltObject::from_float(f);
        prop_assert!(v.is_float());
        let out = v.as_float().unwrap();
        prop_assert_eq!(f.to_bits(), out.to_bits());
    }

    // ------------------------------------------------------------------
    // Distinct tags for distinct types
    // ------------------------------------------------------------------

    /// Two MoltObjects of different type tags must never be bitwise equal,
    /// except in degenerate edge cases (which should not occur).
    #[test]
    fn int_and_bool_never_equal(i in INT_MIN_INLINE..=INT_MAX_INLINE, b in any::<bool>()) {
        let iv = MoltObject::from_int(i);
        let bv = MoltObject::from_bool(b);
        prop_assert_ne!(iv.bits(), bv.bits());
    }
}

// Non-proptest supplementary assertions for singleton / edge values.
#[test]
fn canonical_nan_bits_are_deterministic() {
    let nan1 = MoltObject::from_float(f64::NAN);
    let nan2 = MoltObject::from_float(-f64::NAN);
    // f64::NAN and -f64::NAN have different raw bits but must
    // both canonicalize to the same CANONICAL_NAN_BITS.
    assert_eq!(nan1.bits(), nan2.bits());
    assert_eq!(nan1.bits(), CANONICAL_NAN_BITS);
}

#[test]
fn negative_zero_preserves_sign_bit() {
    let pos = MoltObject::from_float(0.0);
    let neg = MoltObject::from_float(-0.0);
    // IEEE 754: +0.0 and -0.0 have different bit patterns.
    assert_ne!(pos.bits(), neg.bits());
    assert_eq!(pos.as_float().unwrap().to_bits(), 0.0f64.to_bits());
    assert_eq!(neg.as_float().unwrap().to_bits(), (-0.0f64).to_bits());
}

#[test]
fn bool_true_and_false_are_distinct() {
    let t = MoltObject::from_bool(true);
    let f = MoltObject::from_bool(false);
    assert_ne!(t.bits(), f.bits());
    assert_eq!(t.as_bool(), Some(true));
    assert_eq!(f.as_bool(), Some(false));
}

#[test]
fn none_and_pending_are_distinct() {
    let n = MoltObject::none();
    let p = MoltObject::pending();
    assert_ne!(n.bits(), p.bits());
    assert!(n.is_none());
    assert!(!n.is_pending());
    assert!(p.is_pending());
    assert!(!p.is_none());
}

#[test]
fn as_int_unchecked_agrees_with_as_int() {
    // Spot-check a handful of values including boundaries.
    for i in [INT_MIN_INLINE, -1, 0, 1, INT_MAX_INLINE] {
        let v = MoltObject::from_int(i);
        assert_eq!(v.as_int_unchecked(), i);
        assert_eq!(v.as_int(), Some(i));
    }
}
