use super::*;
use molt_wasm_facts::CallableTableAdmission;
use std::num::NonZeroU32;

#[derive(Debug, Default)]
pub(super) struct WasmTableConfig {
    pub(super) base: Option<u32>,
    pub(super) export_name: Option<String>,
}

#[derive(Debug)]
pub(super) enum WasmTableAdmission {
    Empty,
    Publish {
        base: NonZeroU32,
        export_name: String,
    },
}

impl WasmTableAdmission {
    pub(super) fn validate_export(&self, module: &Module) -> Result<()> {
        if let Self::Publish { export_name, .. } = self {
            let Some(ExternType::Func(ty)) = module.get_export(export_name) else {
                bail!(
                    "MOLT_WASM_TABLE_EXPORT_REQUIRED: missing canonical molt_set_wasm_table_base export {export_name}"
                );
            };
            if (ty.params().len() != 1 || !matches!(ty.params().next(), Some(ValType::I64)))
                || ty.results().len() != 0
            {
                bail!(
                    "MOLT_WASM_TABLE_EXPORT_REQUIRED: malformed canonical molt_set_wasm_table_base export {export_name}"
                );
            }
        }
        Ok(())
    }
}

pub(super) fn admit_wasm_table(
    source: &ModuleSource,
    runtime: Option<&ModuleSource>,
    config: WasmTableConfig,
) -> Result<WasmTableAdmission> {
    let admission = molt_wasm_facts::admit_callable_table(source.bytes(), config.base)
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
    match admission {
        CallableTableAdmission::Empty => Ok(WasmTableAdmission::Empty),
        CallableTableAdmission::Publish { base } => {
            let export_name = config.export_name.filter(|name| !name.is_empty()).context(
                "MOLT_WASM_TABLE_EXPORT_REQUIRED: ABI must name canonical molt_set_wasm_table_base export",
            )?;
            Ok(WasmTableAdmission::Publish { base, export_name })
        }
    }
}
