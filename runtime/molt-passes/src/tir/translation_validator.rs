//! TIR translation-validation checks owned by `molt-check`.
//!
//! R3a's current validator is deliberately narrow: for each per-pass
//! before/after fact profile, a surviving `ValueId` may only move upward in the
//! representation proof order. This is not [`crate::repr::Repr::join`]'s
//! semantic carrier order; it is the validation order that allows passes to add
//! raw-carrier proof while forbidding carrier-widening drift.

use std::collections::BTreeMap;

use serde::Serialize;

pub const REPR_LATTICE_CHECK: &str = "repr_lattice_monotonic";

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct ReprTransitionViolation {
    pub value_id: String,
    pub before: String,
    pub after: String,
    pub reason: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct TranslationValidation {
    pub check: &'static str,
    pub passed: bool,
    pub violations: Vec<ReprTransitionViolation>,
}

impl TranslationValidation {
    pub fn panic_message(&self, function: &str, pass_name: &str) -> String {
        let details = self
            .violations
            .iter()
            .take(10)
            .map(|violation| {
                format!(
                    "{}: {}->{} {}",
                    violation.value_id, violation.before, violation.after, violation.reason
                )
            })
            .collect::<Vec<_>>()
            .join("; ");
        format!(
            "[molt-check] TIR translation validation failed in {function}/{pass_name}: {} violation(s) for {}: {details}",
            self.violations.len(),
            self.check,
        )
    }
}

pub fn enabled() -> bool {
    std::env::var("MOLT_CHECK_TIR_TRANSLATION").as_deref() == Ok("1")
}

pub fn validate_repr_lattice_monotonic(
    before: &BTreeMap<String, String>,
    after: &BTreeMap<String, String>,
) -> TranslationValidation {
    let mut violations = Vec::new();
    for (value_id, before_repr) in before {
        let Some(after_repr) = after.get(value_id) else {
            continue;
        };
        if let Some(reason) = repr_transition_violation(before_repr, after_repr) {
            violations.push(ReprTransitionViolation {
                value_id: value_id.clone(),
                before: before_repr.clone(),
                after: after_repr.clone(),
                reason,
            });
        }
    }
    TranslationValidation {
        check: REPR_LATTICE_CHECK,
        passed: violations.is_empty(),
        violations,
    }
}

fn repr_transition_violation(before: &str, after: &str) -> Option<String> {
    if before == after || before == "Never" || before == "DynBox" {
        return None;
    }
    let Some(before_family) = repr_family(before) else {
        return Some(format!("unknown before repr {before}"));
    };
    let Some(after_family) = repr_family(after) else {
        return Some(format!("unknown after repr {after}"));
    };
    if after == "DynBox" {
        return Some("moves to DynBox".to_string());
    }
    if before_family != after_family {
        return Some(format!("crosses {before_family}->{after_family}"));
    }
    let before_rank = repr_proof_rank(before).expect("known repr has rank");
    let after_rank = repr_proof_rank(after).expect("known repr has rank");
    if after_rank < before_rank {
        return Some("moves downward in repr proof order".to_string());
    }
    None
}

fn repr_family(repr: &str) -> Option<&'static str> {
    match repr {
        "Never" => Some("bottom"),
        "DynBox" => Some("dyn"),
        "MaybeBigInt" | "RawI64Safe" | "RawI64FullDeopt" => Some("int"),
        "Bool" => Some("bool"),
        "FloatUnboxed" => Some("float"),
        _ => None,
    }
}

fn repr_proof_rank(repr: &str) -> Option<u8> {
    match repr {
        "Never" => Some(0),
        "DynBox" => Some(1),
        "MaybeBigInt" => Some(2),
        "Bool" => Some(2),
        "FloatUnboxed" => Some(2),
        "RawI64Safe" => Some(3),
        "RawI64FullDeopt" => Some(4),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reprs(values: &[(&str, &str)]) -> BTreeMap<String, String> {
        values
            .iter()
            .map(|(value, repr)| ((*value).to_string(), (*repr).to_string()))
            .collect()
    }

    #[test]
    fn raw_carrier_proof_promotion_passes() {
        let validation = validate_repr_lattice_monotonic(
            &reprs(&[("0", "MaybeBigInt"), ("1", "RawI64Safe")]),
            &reprs(&[("0", "RawI64Safe"), ("1", "RawI64FullDeopt")]),
        );

        assert!(validation.passed);
    }

    #[test]
    fn carrier_widening_drift_fails() {
        let validation = validate_repr_lattice_monotonic(
            &reprs(&[("7", "RawI64Safe")]),
            &reprs(&[("7", "MaybeBigInt")]),
        );

        assert!(!validation.passed);
        assert_eq!(validation.violations.len(), 1);
        assert_eq!(validation.violations[0].value_id, "7");
        assert_eq!(validation.violations[0].before, "RawI64Safe");
        assert_eq!(validation.violations[0].after, "MaybeBigInt");
    }

    #[test]
    fn scalar_family_drift_fails() {
        let validation = validate_repr_lattice_monotonic(
            &reprs(&[("3", "RawI64Safe")]),
            &reprs(&[("3", "FloatUnboxed")]),
        );

        assert!(!validation.passed);
        assert!(
            validation.violations[0]
                .reason
                .contains("crosses int->float")
        );
    }
}
