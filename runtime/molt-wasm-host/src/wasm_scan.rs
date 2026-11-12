use super::*;

/// Admit the callable-table attestation of the main module and, in a split
/// deployment, of the shared runtime. Hosts publish no table base: runtime
/// callables are keys the runtime resolves itself (HF-699 K3).
pub(super) fn admit_callable_tables(
    source: &ModuleSource,
    runtime: Option<&ModuleSource>,
) -> Result<()> {
    molt_wasm_facts::scan_attested_callable_table(source.bytes())
        .map_err(wasmtime::Error::msg)
        .with_context(|| format!("admit callable-table facts from {:?}", source.path()))?;
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
