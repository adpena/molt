use molt_backend::json_boundary::{expect_object, optional_bool, optional_string, required_string};
use serde_json::Value as JsonValue;

use super::model::{
    DaemonArtifactCacheKeys, DaemonCacheContext, DaemonCacheKey, DaemonIrSource, DaemonJobRequest,
    DaemonJobTarget,
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
        let environment = molt_backend::backend_environment::current_semantic_environment()?;
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
        if obj.contains_key("is_wasm") {
            // Every job is native; a wasm32 program names its target triple.
            return Err(format!(
                "{ctx}.is_wasm is retired; a wasm32 job is a native job with target_triple"
            ));
        }
        let native_output_kind = optional_string(obj, "native_output_kind", ctx)?;
        let target = native_job_target(obj, native_output_kind.as_deref(), ctx)?;
        if obj.contains_key("ir") {
            return Err(format!(
                "{ctx}.ir is retired; compile jobs require a MessagePack ir_path"
            ));
        }
        let ir_path = optional_string(obj, "ir_path", ctx)?;
        let probe = optional_bool(obj, "probe_cache_only", ctx)?.unwrap_or(false);
        let function_digests = optional_string(obj, "function_digests", ctx)?;
        let ir_source = match (probe, ir_path, function_digests) {
            (true, None, Some(digests)) if !obj.contains_key("ir_path") => DaemonIrSource::Probe(
                molt_ir::addressed_program::FunctionDigest::parse_concatenated_hex(&digests)
                    .map_err(|error| format!("{ctx}.function_digests: {error}"))?,
            ),
            (false, Some(path), None)
                if !path.is_empty() && !obj.contains_key("function_digests") =>
            {
                DaemonIrSource::Lease(path)
            }
            (true, None, None) => {
                return Err(format!("{ctx} cache probe requires function_digests"));
            }
            (true, _, _) => return Err(format!("{ctx} cache probe must not contain ir_path")),
            (false, Some(_), Some(_)) => {
                return Err(format!(
                    "{ctx} compile names its functions in the ir_path lease, not function_digests"
                ));
            }
            (false, _, _) => return Err(format!("{ctx} compile requires a nonempty ir_path")),
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
            ir_source,
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

#[cfg(test)]
#[path = "job_tests.rs"]
mod tests;
