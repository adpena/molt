use std::collections::HashSet;

use crate::audit::{AuditArgs, audit_capability_decision};
use crate::{PyToken, runtime_state};
use molt_runtime_core::host_capabilities_generated::{
    DEFAULT_CAPABILITY_TIER, MAXIMUM_BUILTIN_CAPABILITY_TIER, OperationId, grants_for_tier,
    minimum_tier_for,
};

fn load_capabilities() -> HashSet<String> {
    let tier = std::env::var("MOLT_CAPABILITY_TIER")
        .ok()
        .map(|t| t.trim().to_ascii_lowercase())
        .unwrap_or_else(|| DEFAULT_CAPABILITY_TIER.to_string());

    let mut set: HashSet<String> = HashSet::new();
    // Invalid tier names fail closed. CLI/config validation reports the typo;
    // a directly embedded runtime must never replace it with ambient grants.
    if let Some(tier_grants) = grants_for_tier(&tier) {
        for &cap in tier_grants {
            set.insert(cap.to_string());
        }
    }

    let caps = std::env::var("MOLT_CAPABILITIES").unwrap_or_default();
    for cap in caps.split(',') {
        let cap = cap.trim();
        if !cap.is_empty() {
            set.insert(cap.to_string());
        }
    }
    set
}

fn load_maximum_builtin_tier_selected() -> bool {
    std::env::var("MOLT_CAPABILITY_TIER")
        .ok()
        .map(|tier| {
            tier.trim()
                .eq_ignore_ascii_case(MAXIMUM_BUILTIN_CAPABILITY_TIER)
        })
        .unwrap_or(false)
}

fn load_execution_target() -> String {
    if !cfg!(target_arch = "wasm32") {
        return "native".to_owned();
    }
    std::env::var("MOLT_EXECUTION_TARGET")
        .map(|target| target.trim().to_ascii_lowercase())
        .unwrap_or_else(|_| "wasi".to_owned())
}

pub(crate) fn is_trusted(_py: &PyToken<'_>) -> bool {
    *runtime_state(_py)
        .trusted
        .get_or_init(load_maximum_builtin_tier_selected)
}

pub(crate) fn has_capability(_py: &PyToken<'_>, name: &str) -> bool {
    let caps = runtime_state(_py)
        .capabilities
        .get_or_init(load_capabilities);
    caps.contains(name)
}

#[inline]
fn operation_supported_here(_py: &PyToken<'_>, operation: OperationId) -> bool {
    let target = runtime_state(_py)
        .execution_target
        .get_or_init(load_execution_target);
    let python = crate::object::ops_sys::runtime_target_python_info(runtime_state(_py));
    operation.supports_target(
        target,
        std::env::consts::OS,
        std::env::consts::ARCH,
        python.major,
        python.minor,
    )
}

fn operation_grants_allowed(_py: &PyToken<'_>, operation: OperationId, args: AuditArgs) -> bool {
    let mut all_allowed = true;
    for capability in operation.required_capabilities() {
        let capability = capability.as_str();
        let allowed = has_capability(_py, capability);
        audit_capability_decision(operation.as_str(), capability, args.clone(), allowed);
        all_allowed &= allowed;
    }
    all_allowed
}

/// Evaluate and audit one generated host operation against its target cell and exact grants.
pub(crate) fn operation_allowed(
    _py: &PyToken<'_>,
    operation: OperationId,
    args: AuditArgs,
) -> bool {
    operation_supported_here(_py, operation) && operation_grants_allowed(_py, operation, args)
}

/// Require one generated host operation, preserving the exact denied grant in
/// the diagnostic and the same operation/capability pair in the audit stream.
pub(crate) fn require_operation<T: crate::builtins::exceptions::ExceptionSentinel>(
    _py: &PyToken<'_>,
    operation: OperationId,
    args: AuditArgs,
) -> Result<(), T> {
    let target = runtime_state(_py)
        .execution_target
        .get_or_init(load_execution_target);
    let platform = std::env::consts::OS;
    let architecture = std::env::consts::ARCH;
    if !operation_supported_here(_py, operation) {
        return Err(crate::raise_exception::<T>(
            _py,
            "RuntimeError",
            &format!(
                "operation '{}' is unavailable for target={target}, platform={platform}, architecture={architecture}",
                operation.as_str()
            ),
        ));
    }
    if operation_grants_allowed(_py, operation, args) {
        return Ok(());
    }
    let missing = operation
        .required_capabilities()
        .iter()
        .map(|capability| capability.as_str())
        .filter(|capability| !has_capability(_py, capability))
        .collect::<Vec<_>>()
        .join(", ");
    Err(crate::raise_exception::<T>(
        _py,
        "PermissionError",
        &format!(
            "missing [{missing}] capabilities for '{}'",
            operation.as_str()
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_operation_target_gates_apply_to_boolean_checks() {
        assert!(OperationId::SelectPoll.supports_target("native", "windows", "x86_64", 3, 12));
        assert!(OperationId::SelectEpoll.supports_target("native", "linux", "x86_64", 3, 14));
        assert!(!OperationId::SelectEpoll.supports_target("browser", "linux", "x86_64", 3, 12));
        assert!(!OperationId::SelectPoll.supports_target("unknown", "linux", "x86_64", 3, 12));
        assert!(!OperationId::SelectPoll.supports_target("native", "linux", "x86_64", 3, 15));
    }
}

/// Suggest the minimum tier or env var needed to grant a missing capability.
/// Raise a PermissionError with an actionable message including the
/// capability name and a finite generated tier suggestion.
pub(crate) fn raise_capability_denied<T: crate::builtins::exceptions::ExceptionSentinel>(
    _py: &crate::concurrency::PyToken<'_>,
    cap: &str,
) -> T {
    let hint = capability_fix_hint(cap);
    crate::raise_exception::<T>(
        _py,
        "PermissionError",
        &format!("missing '{cap}' capability. {hint}"),
    )
}

/// Suggest the minimum tier or env var needed to grant a missing capability.
pub(crate) fn capability_fix_hint(name: &str) -> String {
    if let Some(tier) = minimum_tier_for(name) {
        return format!("Grant MOLT_CAPABILITIES={name} or select MOLT_CAPABILITY_TIER={tier}");
    }
    format!("Grant MOLT_CAPABILITIES={name}")
}
