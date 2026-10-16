//! One compiler stage-audit switch for passes and backends.
//!
//! `MOLT_STAGE_AUDIT` names the outermost stage to audit, and every stage it
//! contains is audited too: `wasm` audits WASM planning and emission, the TIR
//! module pipeline and drop insertion; `module` audits the last two; `drop`
//! audits drop insertion alone.

use std::sync::Once;

/// The audited compiler stages, innermost first.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum AuditStage {
    /// Drop insertion inside each function.
    Drop,
    /// The TIR module pipeline.
    Module,
    /// WASM planning and emission.
    Wasm,
}

impl AuditStage {
    fn parse(raw: &str) -> Option<Self> {
        match raw.trim() {
            "drop" => Some(Self::Drop),
            "module" => Some(Self::Module),
            "wasm" => Some(Self::Wasm),
            _ => None,
        }
    }
}

static UNKNOWN_STAGE_WARNING: Once = Once::new();

/// Whether `MOLT_STAGE_AUDIT` selects `stage` in this process.
///
/// Unset or empty audits nothing. An unknown value audits nothing and warns
/// once, so a misspelled stage is not mistaken for a quiet run.
pub fn stage_audit_enabled(stage: AuditStage) -> bool {
    let Ok(raw) =
        crate::backend_environment::var(crate::backend_environment::BackendEnv::MOLT_STAGE_AUDIT)
    else {
        return false;
    };
    if raw.trim().is_empty() {
        return false;
    }
    match AuditStage::parse(&raw) {
        Some(selected) => stage <= selected,
        None => {
            UNKNOWN_STAGE_WARNING.call_once(|| {
                eprintln!(
                    "molt: MOLT_STAGE_AUDIT={raw:?} names no stage; use drop, module or wasm"
                );
            });
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AuditStage;

    #[test]
    fn a_selected_stage_audits_the_stages_it_contains() {
        let selected = AuditStage::parse(" module ").expect("module is a stage");
        assert!(AuditStage::Drop <= selected);
        assert!(AuditStage::Module <= selected);
        assert!(AuditStage::Wasm > selected);
        assert_eq!(AuditStage::parse("1"), None);
    }
}
