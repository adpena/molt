//! Kani bounded model-checking harnesses for the NaN-boxed object model.
//!
//! Each harness uses `kani::any()` to exhaustively (within bounds) verify
//! invariants that must hold for *every* possible input.
//!
//! Run with: `cargo kani --tests -p molt-lang-obj-model`

// The `kani` cfg is only defined when running under `cargo kani`.
#![allow(unexpected_cfgs)]

#[cfg(kani)]
mod kani_proofs {
    use molt_lang_obj_model::MoltObject;

    // ---------------------------------------------------------------
    // Constants (mirrors of crate-internal values for assertion)
    // ---------------------------------------------------------------
    const CANONICAL_NAN_BITS: u64 = 0x7ff0_0000_0000_0001;
    const INT_MIN_INLINE: i64 = -(1i64 << 46);
    const INT_MAX_INLINE: i64 = (1i64 << 46) - 1;

    // =================================================================
    // Float harnesses
    // =================================================================

    /// Any f64 round-trips through from_float / as_float (or is canonicalized NaN).
    #[kani::proof]
    fn float_roundtrip() {
        let f: f64 = kani::any();
        let obj = MoltObject::from_float(f);
        if f.is_nan() {
            assert_eq!(obj.bits(), CANONICAL_NAN_BITS);
        } else {
            assert!(obj.is_float());
            let out = obj.as_float().unwrap();
            assert_eq!(f.to_bits(), out.to_bits());
        }
    }

    /// A non-NaN float is never mistaken for any tagged type.
    #[kani::proof]
    fn float_exclusivity() {
        let f: f64 = kani::any();
        kani::assume(!f.is_nan());
        let obj = MoltObject::from_float(f);
        assert!(obj.is_float());
        assert!(!obj.is_int());
        assert!(!obj.is_bool());
        assert!(!obj.is_none());
        assert!(!obj.is_ptr());
        assert!(!obj.is_pending());
    }

    // =================================================================
    // Int harnesses
    // =================================================================

    /// Any i64 in the inline range round-trips through from_int / as_int.
    #[kani::proof]
    fn int_roundtrip() {
        let i: i64 = kani::any();
        kani::assume(i >= INT_MIN_INLINE && i <= INT_MAX_INLINE);
        let obj = MoltObject::from_int(i);
        assert!(obj.is_int());
        assert_eq!(obj.as_int(), Some(i));
    }

    /// An int is never mistaken for any other type.
    #[kani::proof]
    fn int_exclusivity() {
        let i: i64 = kani::any();
        kani::assume(i >= INT_MIN_INLINE && i <= INT_MAX_INLINE);
        let obj = MoltObject::from_int(i);
        assert!(obj.is_int());
        assert!(!obj.is_float());
        assert!(!obj.is_bool());
        assert!(!obj.is_none());
        assert!(!obj.is_ptr());
        assert!(!obj.is_pending());
    }

    /// Negative int round-trip preserves sign via sign-extension.
    #[kani::proof]
    fn negative_int_roundtrip() {
        let i: i64 = kani::any();
        kani::assume(i >= INT_MIN_INLINE && i < 0);
        let obj = MoltObject::from_int(i);
        assert!(obj.is_int());
        assert_eq!(obj.as_int(), Some(i));
    }

    /// as_int_unchecked agrees with as_int for valid inline ints.
    #[kani::proof]
    fn int_unchecked_agrees() {
        let i: i64 = kani::any();
        kani::assume(i >= INT_MIN_INLINE && i <= INT_MAX_INLINE);
        let obj = MoltObject::from_int(i);
        assert_eq!(obj.as_int_unchecked(), obj.as_int().unwrap());
    }

    /// Int values carry the QNAN prefix.
    #[kani::proof]
    fn int_has_qnan_prefix() {
        let i: i64 = kani::any();
        kani::assume(i >= INT_MIN_INLINE && i <= INT_MAX_INLINE);
        let obj = MoltObject::from_int(i);
        assert_eq!(obj.bits() & 0x7ff8_0000_0000_0000, 0x7ff8_0000_0000_0000);
    }

    // =================================================================
    // Bool harnesses
    // =================================================================

    /// Bool round-trips through from_bool / as_bool.
    #[kani::proof]
    fn bool_roundtrip() {
        let b: bool = kani::any();
        let obj = MoltObject::from_bool(b);
        assert!(obj.is_bool());
        assert_eq!(obj.as_bool(), Some(b));
    }

    /// A bool is never mistaken for any other type.
    #[kani::proof]
    fn bool_exclusivity() {
        let b: bool = kani::any();
        let obj = MoltObject::from_bool(b);
        assert!(obj.is_bool());
        assert!(!obj.is_float());
        assert!(!obj.is_int());
        assert!(!obj.is_none());
        assert!(!obj.is_ptr());
        assert!(!obj.is_pending());
    }

    /// true and false produce distinct bit patterns.
    #[kani::proof]
    fn bool_true_false_distinct() {
        let t = MoltObject::from_bool(true);
        let f = MoltObject::from_bool(false);
        assert_ne!(t.bits(), f.bits());
    }

    // =================================================================
    // None harnesses
    // =================================================================

    /// None is a singleton with a stable bit pattern.
    #[kani::proof]
    fn none_singleton() {
        let a = MoltObject::none();
        let b = MoltObject::none();
        assert_eq!(a.bits(), b.bits());
        assert!(a.is_none());
    }

    /// None is never mistaken for any other type.
    #[kani::proof]
    fn none_exclusivity() {
        let obj = MoltObject::none();
        assert!(obj.is_none());
        assert!(!obj.is_float());
        assert!(!obj.is_int());
        assert!(!obj.is_bool());
        assert!(!obj.is_ptr());
        assert!(!obj.is_pending());
    }

    // =================================================================
    // Pending harnesses
    // =================================================================

    /// Pending round-trip: from_pending produces is_pending.
    #[kani::proof]
    fn pending_roundtrip() {
        let obj = MoltObject::pending();
        assert!(obj.is_pending());
    }

    /// Pending is never mistaken for any other type.
    #[kani::proof]
    fn pending_exclusivity() {
        let obj = MoltObject::pending();
        assert!(obj.is_pending());
        assert!(!obj.is_float());
        assert!(!obj.is_int());
        assert!(!obj.is_bool());
        assert!(!obj.is_none());
        assert!(!obj.is_ptr());
    }

    /// Pending and None are distinct.
    #[kani::proof]
    fn pending_and_none_distinct() {
        let p = MoltObject::pending();
        let n = MoltObject::none();
        assert_ne!(p.bits(), n.bits());
    }

    // =================================================================
    // from_bits identity harnesses
    // =================================================================

    /// from_bits(v.bits()) is identity for floats.
    #[kani::proof]
    fn from_bits_identity_float() {
        let f: f64 = kani::any();
        let v = MoltObject::from_float(f);
        let v2 = MoltObject::from_bits(v.bits());
        assert_eq!(v, v2);
    }

    /// from_bits(v.bits()) is identity for ints.
    #[kani::proof]
    fn from_bits_identity_int() {
        let i: i64 = kani::any();
        kani::assume(i >= INT_MIN_INLINE && i <= INT_MAX_INLINE);
        let v = MoltObject::from_int(i);
        let v2 = MoltObject::from_bits(v.bits());
        assert_eq!(v, v2);
    }

    /// from_bits(v.bits()) is identity for bools.
    #[kani::proof]
    fn from_bits_identity_bool() {
        let b: bool = kani::any();
        let v = MoltObject::from_bool(b);
        let v2 = MoltObject::from_bits(v.bits());
        assert_eq!(v, v2);
    }

    /// from_bits(v.bits()) is identity for none.
    #[kani::proof]
    fn from_bits_identity_none() {
        let v = MoltObject::none();
        let v2 = MoltObject::from_bits(v.bits());
        assert_eq!(v, v2);
    }

    /// from_bits(v.bits()) is identity for pending.
    #[kani::proof]
    fn from_bits_identity_pending() {
        let v = MoltObject::pending();
        let v2 = MoltObject::from_bits(v.bits());
        assert_eq!(v, v2);
    }

    // =================================================================
    // from_bits preserves type for all constructors
    // =================================================================

    /// from_bits preserves the type tag: reconstructing from bits yields
    /// the same type predicates as the original.
    #[kani::proof]
    fn from_bits_preserves_type_for_all_constructors() {
        // Pick a constructor via a selector byte
        let sel: u8 = kani::any();
        kani::assume(sel < 5);

        match sel {
            0 => {
                let f: f64 = kani::any();
                let orig = MoltObject::from_float(f);
                let rebuilt = MoltObject::from_bits(orig.bits());
                assert_eq!(rebuilt.is_float(), orig.is_float());
                assert_eq!(rebuilt.is_int(), orig.is_int());
                assert_eq!(rebuilt.is_bool(), orig.is_bool());
                assert_eq!(rebuilt.is_none(), orig.is_none());
                assert_eq!(rebuilt.is_pending(), orig.is_pending());
                assert_eq!(rebuilt.is_ptr(), orig.is_ptr());
            }
            1 => {
                let i: i64 = kani::any();
                kani::assume(i >= INT_MIN_INLINE && i <= INT_MAX_INLINE);
                let orig = MoltObject::from_int(i);
                let rebuilt = MoltObject::from_bits(orig.bits());
                assert_eq!(rebuilt.is_int(), orig.is_int());
                assert_eq!(rebuilt.is_float(), orig.is_float());
            }
            2 => {
                let b: bool = kani::any();
                let orig = MoltObject::from_bool(b);
                let rebuilt = MoltObject::from_bits(orig.bits());
                assert_eq!(rebuilt.is_bool(), orig.is_bool());
                assert_eq!(rebuilt.is_float(), orig.is_float());
            }
            3 => {
                let orig = MoltObject::none();
                let rebuilt = MoltObject::from_bits(orig.bits());
                assert_eq!(rebuilt.is_none(), orig.is_none());
            }
            4 => {
                let orig = MoltObject::pending();
                let rebuilt = MoltObject::from_bits(orig.bits());
                assert_eq!(rebuilt.is_pending(), orig.is_pending());
            }
            _ => unreachable!(),
        }
    }

    // =================================================================
    // bits() injectivity for same-type values
    // =================================================================

    /// Two different inline ints must produce different bit patterns.
    #[kani::proof]
    fn bits_injective_for_ints() {
        let a: i64 = kani::any();
        let b: i64 = kani::any();
        kani::assume(a >= INT_MIN_INLINE && a <= INT_MAX_INLINE);
        kani::assume(b >= INT_MIN_INLINE && b <= INT_MAX_INLINE);
        kani::assume(a != b);
        let obj_a = MoltObject::from_int(a);
        let obj_b = MoltObject::from_int(b);
        assert_ne!(obj_a.bits(), obj_b.bits());
    }

    /// Two different non-NaN floats must produce different bit patterns.
    #[kani::proof]
    fn bits_injective_for_floats() {
        let a: f64 = kani::any();
        let b: f64 = kani::any();
        kani::assume(!a.is_nan() && !b.is_nan());
        kani::assume(a.to_bits() != b.to_bits());
        let obj_a = MoltObject::from_float(a);
        let obj_b = MoltObject::from_float(b);
        assert_ne!(obj_a.bits(), obj_b.bits());
    }

    // =================================================================
    // MoltObject::eq reflexivity
    // =================================================================

    /// MoltObject::eq is reflexive for all constructed objects.
    #[kani::proof]
    fn eq_reflexive_int() {
        let i: i64 = kani::any();
        kani::assume(i >= INT_MIN_INLINE && i <= INT_MAX_INLINE);
        let obj = MoltObject::from_int(i);
        assert_eq!(obj, obj);
    }

    #[kani::proof]
    fn eq_reflexive_float() {
        let f: f64 = kani::any();
        let obj = MoltObject::from_float(f);
        // NaN is special: the derived PartialEq compares bits, so
        // canonical NaN == canonical NaN.
        assert_eq!(obj, obj);
    }

    #[kani::proof]
    fn eq_reflexive_bool() {
        let b: bool = kani::any();
        let obj = MoltObject::from_bool(b);
        assert_eq!(obj, obj);
    }

    #[kani::proof]
    fn eq_reflexive_none() {
        let obj = MoltObject::none();
        assert_eq!(obj, obj);
    }

    #[kani::proof]
    fn eq_reflexive_pending() {
        let obj = MoltObject::pending();
        assert_eq!(obj, obj);
    }

    // =================================================================
    // from_float canonical NaN
    // =================================================================

    /// All NaN variants (positive, negative, signaling, quiet) produce
    /// the same canonical MoltObject.
    #[kani::proof]
    fn from_float_canonical_nan() {
        let bits: u64 = kani::any();
        let f = f64::from_bits(bits);
        kani::assume(f.is_nan());
        let obj = MoltObject::from_float(f);
        assert_eq!(obj.bits(), CANONICAL_NAN_BITS);
    }

    // =================================================================
    // Cross-type non-collision
    // =================================================================

    /// Int and bool never collide.
    #[kani::proof]
    fn int_bool_no_collision() {
        let i: i64 = kani::any();
        kani::assume(i >= INT_MIN_INLINE && i <= INT_MAX_INLINE);
        let b: bool = kani::any();
        assert_ne!(
            MoltObject::from_int(i).bits(),
            MoltObject::from_bool(b).bits()
        );
    }

    /// Int and none never collide.
    #[kani::proof]
    fn int_none_no_collision() {
        let i: i64 = kani::any();
        kani::assume(i >= INT_MIN_INLINE && i <= INT_MAX_INLINE);
        assert_ne!(MoltObject::from_int(i).bits(), MoltObject::none().bits());
    }

    /// Int and pending never collide.
    #[kani::proof]
    fn int_pending_no_collision() {
        let i: i64 = kani::any();
        kani::assume(i >= INT_MIN_INLINE && i <= INT_MAX_INLINE);
        assert_ne!(MoltObject::from_int(i).bits(), MoltObject::pending().bits());
    }

    /// Bool and none never collide.
    #[kani::proof]
    fn bool_none_no_collision() {
        let b: bool = kani::any();
        assert_ne!(MoltObject::from_bool(b).bits(), MoltObject::none().bits());
    }

    /// Bool and pending never collide.
    #[kani::proof]
    fn bool_pending_no_collision() {
        let b: bool = kani::any();
        assert_ne!(
            MoltObject::from_bool(b).bits(),
            MoltObject::pending().bits()
        );
    }

    // =================================================================
    // Boundary harnesses
    // =================================================================

    /// INT_MIN_INLINE round-trips correctly.
    #[kani::proof]
    fn int_min_boundary() {
        let obj = MoltObject::from_int(INT_MIN_INLINE);
        assert!(obj.is_int());
        assert_eq!(obj.as_int(), Some(INT_MIN_INLINE));
    }

    /// INT_MAX_INLINE round-trips correctly.
    #[kani::proof]
    fn int_max_boundary() {
        let obj = MoltObject::from_int(INT_MAX_INLINE);
        assert!(obj.is_int());
        assert_eq!(obj.as_int(), Some(INT_MAX_INLINE));
    }

    /// Zero round-trips as int.
    #[kani::proof]
    fn int_zero() {
        let obj = MoltObject::from_int(0);
        assert!(obj.is_int());
        assert_eq!(obj.as_int(), Some(0));
    }

    /// -1 round-trips as int (sign extension test).
    #[kani::proof]
    fn int_minus_one() {
        let obj = MoltObject::from_int(-1);
        assert!(obj.is_int());
        assert_eq!(obj.as_int(), Some(-1));
    }

    /// +0.0 and -0.0 are both floats with distinct bits.
    #[kani::proof]
    fn float_signed_zeros() {
        let pos = MoltObject::from_float(0.0);
        let neg = MoltObject::from_float(-0.0);
        assert!(pos.is_float());
        assert!(neg.is_float());
        assert_ne!(pos.bits(), neg.bits());
    }

    /// Infinity round-trips.
    #[kani::proof]
    fn float_infinity_roundtrip() {
        let obj = MoltObject::from_float(f64::INFINITY);
        assert!(obj.is_float());
        assert_eq!(obj.as_float().unwrap(), f64::INFINITY);
    }

    /// Negative infinity round-trips.
    #[kani::proof]
    fn float_neg_infinity_roundtrip() {
        let obj = MoltObject::from_float(f64::NEG_INFINITY);
        assert!(obj.is_float());
        assert_eq!(obj.as_float().unwrap(), f64::NEG_INFINITY);
    }

    // =================================================================
    // Type-accessor safety: wrong-type access returns None
    // =================================================================

    /// as_int on a float returns None.
    #[kani::proof]
    fn as_int_on_float_is_none() {
        let f: f64 = kani::any();
        kani::assume(!f.is_nan());
        let obj = MoltObject::from_float(f);
        assert!(obj.as_int().is_none());
    }

    /// as_float on an int returns None.
    #[kani::proof]
    fn as_float_on_int_is_none() {
        let i: i64 = kani::any();
        kani::assume(i >= INT_MIN_INLINE && i <= INT_MAX_INLINE);
        let obj = MoltObject::from_int(i);
        assert!(obj.as_float().is_none());
    }

    /// as_bool on an int returns None.
    #[kani::proof]
    fn as_bool_on_int_is_none() {
        let i: i64 = kani::any();
        kani::assume(i >= INT_MIN_INLINE && i <= INT_MAX_INLINE);
        let obj = MoltObject::from_int(i);
        assert!(obj.as_bool().is_none());
    }

    /// as_ptr on an int returns None.
    #[kani::proof]
    fn as_ptr_on_int_is_none() {
        let i: i64 = kani::any();
        kani::assume(i >= INT_MIN_INLINE && i <= INT_MAX_INLINE);
        let obj = MoltObject::from_int(i);
        assert!(obj.as_ptr().is_none());
    }
}
