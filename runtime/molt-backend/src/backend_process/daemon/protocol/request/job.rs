#[cfg(feature = "wasm-backend")]
use molt_backend::json_boundary::optional_u32;
use molt_backend::json_boundary::{
    expect_object, optional_bool, optional_string, required_field, required_string,
};
use serde_json::Value as JsonValue;

use super::model::{
    DaemonArtifactCacheKeys, DaemonCacheContext, DaemonCacheKey, DaemonJobRequest, DaemonJobTarget,
};
use std::sync::Arc;

impl DaemonJobRequest {
    /// Move caller strings into one pair of backend-enforced target identities.
    /// Empty keys disable their tier; equal keys share one allocation. This runs
    /// once before cache admission and does not open an IR lease.
    pub(crate) fn take_artifact_cache_keys(&mut self) -> std::io::Result<DaemonArtifactCacheKeys> {
        let module = nonempty_key(std::mem::take(&mut self.cache_key));
        let function = self.function_cache_key.take().and_then(nonempty_key);
        if module.is_none() && function.is_none() {
            return Ok(DaemonArtifactCacheKeys::default());
        }
        use molt_backend::backend_environment::{
            BackendCodegenTarget, current_semantic_environment,
        };
        let target = match &self.target {
            #[cfg(feature = "native-backend")]
            DaemonJobTarget::Native(_) => BackendCodegenTarget::Native,
            #[cfg(feature = "wasm-backend")]
            DaemonJobTarget::Wasm(_) => BackendCodegenTarget::Wasm,
        };
        let environment = current_semantic_environment(target)?;
        let context = Arc::new(DaemonCacheContext {
            config_namespace: self.config_namespace.take(),
            target: self.target.clone(),
            environment,
        });
        let same = module == function;
        let module = module.map(|source| {
            Arc::new(DaemonCacheKey {
                context: Arc::clone(&context),
                source,
            })
        });
        let function = if same {
            module.as_ref().map(Arc::clone)
        } else {
            function.map(|source| Arc::new(DaemonCacheKey { context, source }))
        };
        Ok(DaemonArtifactCacheKeys { module, function })
    }

    pub(crate) fn from_json_value(value: &JsonValue, ctx: &str) -> Result<Self, String> {
        let obj = expect_object(value, ctx)?;
        let is_wasm = required_field(obj, "is_wasm", ctx)?
            .as_bool()
            .ok_or_else(|| format!("{ctx}.is_wasm must be a bool"))?;
        let native_output_kind = optional_string(obj, "native_output_kind", ctx)?;
        if is_wasm && native_output_kind.is_some() {
            return Err(format!("{ctx}.native_output_kind requires a native target"));
        }
        let target = if is_wasm {
            wasm_job_target(obj, ctx)?
        } else {
            native_job_target(obj, native_output_kind.as_deref(), ctx)?
        };
        let ir_path = optional_string(obj, "ir_path", ctx)?;
        if obj.get("ir").is_some_and(|value| !value.is_null()) && ir_path.is_some() {
            return Err(format!(
                "{ctx} must use exactly one IR custody field: ir or ir_path"
            ));
        }
        let ir = match obj.get("ir") {
            None | Some(JsonValue::Null) => None,
            Some(ir_value) => Some(molt_backend::BackendIrDocument::from_json_value(ir_value)?),
        };
        Ok(Self {
            id: required_string(obj, "id", ctx)?,
            target,
            output: required_string(obj, "output", ctx)?,
            cache_key: required_string(obj, "cache_key", ctx)?,
            function_cache_key: optional_string(obj, "function_cache_key", ctx)?,
            skip_module_output_if_synced: optional_bool(obj, "skip_module_output_if_synced", ctx)?
                .unwrap_or(false),
            skip_function_output_if_synced: optional_bool(
                obj,
                "skip_function_output_if_synced",
                ctx,
            )?
            .unwrap_or(false),
            probe_cache_only: optional_bool(obj, "probe_cache_only", ctx)?.unwrap_or(false),
            ir,
            ir_path,
            config_namespace: None,
        })
    }
}

fn nonempty_key(mut key: String) -> Option<String> {
    let end = key.trim_end().len();
    key.truncate(end);
    let start = key.len() - key.trim_start().len();
    key.drain(..start);
    (!key.is_empty()).then_some(key)
}

type JsonObject = serde_json::Map<String, JsonValue>;

#[cfg(feature = "native-backend")]
fn native_job_target(
    obj: &JsonObject,
    native_output_kind: Option<&str>,
    ctx: &str,
) -> Result<DaemonJobTarget, String> {
    Ok(DaemonJobTarget::Native(
        crate::backend_process::NativeTargetOptions {
            target_triple: optional_string(obj, "target_triple", ctx)?,
            output_kind: native_output_kind
                .map(crate::backend_process::NativeArtifactKind::parse)
                .transpose()?
                .unwrap_or_default(),
        },
    ))
}

#[cfg(not(feature = "native-backend"))]
fn native_job_target(
    _: &JsonObject,
    _: Option<&str>,
    ctx: &str,
) -> Result<DaemonJobTarget, String> {
    Err(format!(
        "{ctx} requests a native artifact, but this backend was built without native-backend"
    ))
}

#[cfg(feature = "wasm-backend")]
fn wasm_job_target(obj: &JsonObject, ctx: &str) -> Result<DaemonJobTarget, String> {
    Ok(DaemonJobTarget::Wasm(
        crate::backend_process::WasmTargetOptions {
            link_relocs: optional_bool(obj, "wasm_link", ctx)?.unwrap_or(false),
            data_base: optional_u32(obj, "wasm_data_base", ctx)?,
            table_base: optional_u32(obj, "wasm_table_base", ctx)?,
            split_runtime_app_table_base: optional_u32(
                obj,
                "wasm_split_runtime_app_table_base",
                ctx,
            )?,
        },
    ))
}

#[cfg(not(feature = "wasm-backend"))]
fn wasm_job_target(_: &JsonObject, ctx: &str) -> Result<DaemonJobTarget, String> {
    Err(format!(
        "{ctx} requests a WASM artifact, but this backend was built without wasm-backend"
    ))
}

#[cfg(test)]
#[path = "job_tests.rs"]
mod tests;
