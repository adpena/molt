use std::{fmt, num::NonZeroU32};

use crate::{WasmLinkFacts, scan_wasm_link_facts};

/// A validated callable region. Empty attestations never publish a base.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CallableTableAdmission {
    Empty,
    Publish { base: NonZeroU32 },
}

#[derive(Debug, PartialEq, Eq)]
pub enum CallableTableAdmissionError {
    Missing,
    Malformed(String),
    BaseRequired,
    BaseAboveBinary { declared: u32, binary: u32 },
}

impl fmt::Display for CallableTableAdmissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(
                f,
                "MOLT_WASM_TABLE_ATTESTATION_MISSING: missing callable-table attestation"
            ),
            Self::Malformed(error) => write!(f, "MOLT_WASM_TABLE_ATTESTATION_MALFORMED: {error}"),
            Self::BaseRequired => write!(
                f,
                "MOLT_WASM_TABLE_BASE_REQUIRED: needs a positive callable table base"
            ),
            Self::BaseAboveBinary { declared, binary } => write!(
                f,
                "manifest wasm_table_base {declared} is above binary table base {binary}"
            ),
        }
    }
}

impl std::error::Error for CallableTableAdmissionError {}

/// Both members of a split deployment carry independently validated facts.
pub fn scan_attested_callable_table(
    bytes: &[u8],
) -> Result<WasmLinkFacts, CallableTableAdmissionError> {
    use CallableTableAdmissionError as Error;
    let facts = scan_wasm_link_facts(bytes).map_err(Error::Malformed)?;
    if !facts.callable_table_attestation_present {
        return Err(Error::Missing);
    }
    Ok(facts)
}

/// Reuse the final-module validator, including its generated attestation format.
/// Never infer a region from unrelated element segments. A layout publication
/// owns relocated regions; otherwise the attested first callable slot owns it.
pub fn admit_callable_table(
    bytes: &[u8],
    declared_base: Option<u32>,
) -> Result<CallableTableAdmission, CallableTableAdmissionError> {
    use CallableTableAdmissionError as Error;
    let facts = scan_attested_callable_table(bytes)?;
    let Some(first) = facts.callable_table_entries.first() else {
        return Ok(CallableTableAdmission::Empty);
    };
    let binary = facts.callable_table_layout.map_or(first.slot, |layout| {
        if layout.fixed_prefix_len > 0 {
            layout.fixed_prefix_base
        } else {
            layout.finalized_app_base
        }
    });
    if let Some(declared) = declared_base
        && binary < declared
    {
        return Err(Error::BaseAboveBinary { declared, binary });
    }
    let base = NonZeroU32::new(declared_base.unwrap_or(binary)).ok_or(Error::BaseRequired)?;
    Ok(CallableTableAdmission::Publish { base })
}
