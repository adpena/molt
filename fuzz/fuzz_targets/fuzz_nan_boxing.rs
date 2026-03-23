//! Fuzz target: roundtrip NaN-boxing encode/decode for all value types.
//!
//! Invariants tested:
//! - `from_int(i).as_int() == Some(i)` for all i in the 47-bit signed range
//! - `from_float(f).as_float()` returns `Some` and preserves the value (or
//!   canonicalizes NaN)
//! - `from_bool(b).as_bool() == Some(b)`
//! - `none().is_none()` and `pending().is_pending()`
//! - Type predicates are mutually exclusive

#![no_main]
use libfuzzer_sys::fuzz_target;
use arbitrary::Arbitrary;
use molt_obj_model::MoltObject;

/// Enum that lets the fuzzer choose which NaN-box variant to exercise.
#[derive(Debug, Arbitrary)]
enum ValueInput {
    Int(i64),
    Float(u64),    // raw bits — allows NaN, Inf, subnormals
    Bool(bool),
    None,
    Pending,
    RawBits(u64),  // completely arbitrary bit pattern
}

fuzz_target!(|input: ValueInput| {
    match input {
        ValueInput::Int(i) => {
            let obj = MoltObject::from_int(i);
            assert!(obj.is_int(), "from_int must produce is_int");
            assert!(!obj.is_float());
            assert!(!obj.is_bool());
            assert!(!obj.is_none());
            assert!(!obj.is_ptr());

            // 47-bit signed range: [-(2^46), 2^46 - 1]
            let min = -(1i64 << 46);
            let max = (1i64 << 46) - 1;
            if i >= min && i <= max {
                assert_eq!(
                    obj.as_int(),
                    Some(i),
                    "roundtrip failed for in-range int {i}"
                );
            }
            // Out-of-range values are truncated — just verify no panic.
        }
        ValueInput::Float(bits) => {
            let f = f64::from_bits(bits);
            let obj = MoltObject::from_float(f);
            assert!(obj.is_float(), "from_float must produce is_float");
            assert!(!obj.is_int());
            assert!(!obj.is_bool());
            assert!(!obj.is_none());

            let recovered = obj.as_float().expect("is_float but as_float is None");
            if f.is_nan() {
                assert!(recovered.is_nan(), "NaN input must produce NaN output");
            } else {
                assert_eq!(
                    recovered.to_bits(),
                    f.to_bits(),
                    "float roundtrip failed for bits {bits:#018x}"
                );
            }
        }
        ValueInput::Bool(b) => {
            let obj = MoltObject::from_bool(b);
            assert!(obj.is_bool());
            assert_eq!(obj.as_bool(), Some(b));
            assert!(!obj.is_float());
            assert!(!obj.is_int());
        }
        ValueInput::None => {
            let obj = MoltObject::none();
            assert!(obj.is_none());
            assert!(!obj.is_float());
            assert!(!obj.is_int());
            assert!(!obj.is_bool());
            assert!(!obj.is_ptr());
        }
        ValueInput::Pending => {
            let obj = MoltObject::pending();
            assert!(obj.is_pending());
            assert!(!obj.is_float());
            assert!(!obj.is_int());
            assert!(!obj.is_bool());
            assert!(!obj.is_none());
        }
        ValueInput::RawBits(bits) => {
            // Verify that from_bits never panics and that the type predicates
            // are consistent (at most one is true, except float which covers
            // the non-QNAN space).
            let obj = MoltObject::from_bits(bits);
            let type_count = obj.is_float() as u8
                + obj.is_int() as u8
                + obj.is_bool() as u8
                + obj.is_none() as u8
                + obj.is_ptr() as u8
                + obj.is_pending() as u8;
            assert!(
                type_count <= 1,
                "multiple type predicates true for bits {bits:#018x}: \
                 float={}, int={}, bool={}, none={}, ptr={}, pending={}",
                obj.is_float(),
                obj.is_int(),
                obj.is_bool(),
                obj.is_none(),
                obj.is_ptr(),
                obj.is_pending(),
            );
        }
    }
});
