//! Source call-instruction custody, shared by IR admission and every consumer.
//!
//! The existing `callargs_new.s_value` wire projection omits ordinary stack
//! calls and spells expanded calls `expanded`. Decode that projection here;
//! backends and optimizers consume the typed form instead of interpreting it.

use crate::OpIR;
use crate::tir::ops::{AttrValue, TirOp};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CallArgumentForm {
    Stack,
    Expanded,
}

impl CallArgumentForm {
    pub fn from_wire(value: Option<&str>) -> Result<Self, String> {
        match value {
            None => Ok(Self::Stack),
            Some("expanded") => Ok(Self::Expanded),
            Some(value) => Err(format!("callargs_new carries unknown call form `{value}`")),
        }
    }

    pub const fn runtime_constructor(self) -> &'static str {
        match self {
            Self::Stack => "molt_callargs_new",
            Self::Expanded => "molt_callargs_new_expanded",
        }
    }
}

impl OpIR {
    /// Decode the custody of a `callargs_new` operation.
    pub fn call_argument_form(&self) -> Result<CallArgumentForm, String> {
        CallArgumentForm::from_wire(self.s_value.as_deref())
    }
}

impl TirOp {
    /// Decode the same fact after the operation is preserved in TIR.
    pub fn call_argument_form(&self) -> Result<CallArgumentForm, String> {
        let value = match self.attrs.get("s_value") {
            None => None,
            Some(AttrValue::Str(value)) => Some(value.as_str()),
            Some(_) => return Err("callargs_new call form must be a string".into()),
        };
        CallArgumentForm::from_wire(value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn call_argument_form_admission_preserves_custody_and_rejects_unknown_forms() {
        for (wire, expected) in [
            (None, CallArgumentForm::Stack),
            (Some("expanded"), CallArgumentForm::Expanded),
        ] {
            let ir = serde_json::json!({"functions": [{
                "name": "f", "params": [], "return_abi": "void",
                "ops": [{"kind": "callargs_new", "out": "builder", "s_value": wire}]
            }]});
            let admitted = crate::SimpleIR::from_json_value(&ir).unwrap();
            assert_eq!(
                admitted.functions[0].ops[0].call_argument_form().unwrap(),
                expected
            );
        }
        let invalid = serde_json::json!({"functions": [{
            "name": "f", "params": [], "return_abi": "void",
            "ops": [{"kind": "callargs_new", "out": "builder", "s_value": "typo"}]
        }]});
        assert!(
            crate::SimpleIR::from_json_value(&invalid)
                .unwrap_err()
                .contains("unknown call form")
        );
    }
}
