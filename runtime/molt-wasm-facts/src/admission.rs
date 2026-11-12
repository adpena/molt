use std::fmt;

use crate::{WasmLinkFacts, scan_wasm_link_facts};

/// Why a module's callable-table attestation is not admitted. A host admits
/// the attestation only: runtime callables are keys, so no table base is
/// published to the runtime (HF-699 K3).
#[derive(Debug, PartialEq, Eq)]
pub enum CallableTableAdmissionError {
    Missing,
    Malformed(String),
}

impl fmt::Display for CallableTableAdmissionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Missing => write!(
                f,
                "MOLT_WASM_TABLE_ATTESTATION_MISSING: missing callable-table attestation"
            ),
            Self::Malformed(error) => write!(f, "MOLT_WASM_TABLE_ATTESTATION_MALFORMED: {error}"),
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
