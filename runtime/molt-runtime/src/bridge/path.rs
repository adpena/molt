use std::path::PathBuf;

use crate::audit::{AuditArgs, audit_capability_decision};
use molt_runtime_core::prelude::*;

pub fn has_capability(_py: &CoreGilToken, name: &str) -> bool {
    crate::with_gil_entry_nopanic!(py, { crate::has_capability(py, name) })
}

pub enum AuditArg {
    None,
    Path(String),
}

pub fn audit_capability(
    _py: &CoreGilToken,
    operation: &'static str,
    capability: &'static str,
    arg: AuditArg,
) -> bool {
    let allowed = has_capability(_py, capability);
    let args = match arg {
        AuditArg::None => AuditArgs::None,
        AuditArg::Path(path) => AuditArgs::Path(path),
    };
    audit_capability_decision(operation, capability, args, allowed);
    allowed
}

pub fn path_from_bits(_py: &CoreGilToken, bits: u64) -> Result<PathBuf, String> {
    crate::with_gil_entry_nopanic!(py, { crate::path_from_bits(py, bits) })
}

pub fn type_name(_py: &CoreGilToken, obj: MoltObject) -> String {
    crate::with_gil_entry_nopanic!(py, { crate::type_name(py, obj).into_owned() })
}
