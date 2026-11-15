use super::*;

/// Admit the callable-table attestation of the main module and, in a split
/// deployment, of the shared runtime. Hosts publish no table base: runtime
/// callables are keys the runtime resolves itself (HF-699 K3).
///
/// A linked module attests its table only when its producer laid the table
/// out for host dispatch (the WASM backend). A standard LLVM wasm32 module's
/// table is wasm-ld's own and the runtime calls its slots directly (HF-699
/// K2), so no host reads it: its attestation is absent, never partial, and a
/// malformed one is still refused.
pub(super) fn admit_callable_tables(
    source: &ModuleSource,
    runtime: Option<&ModuleSource>,
) -> Result<()> {
    match molt_wasm_facts::scan_attested_callable_table(source.bytes()) {
        Ok(_) => {}
        Err(molt_wasm_facts::CallableTableAdmissionError::Missing) if runtime.is_none() => {}
        Err(error) => {
            return Err(wasmtime::Error::msg(error))
                .with_context(|| format!("admit callable-table facts from {:?}", source.path()));
        }
    }
    if let Some(runtime) = runtime {
        molt_wasm_facts::scan_attested_callable_table(runtime.bytes())
            .map_err(wasmtime::Error::msg)
            .with_context(|| {
                format!(
                    "admit runtime callable-table facts from {:?}",
                    runtime.path()
                )
            })?;
    }
    Ok(())
}
