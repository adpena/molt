#[cfg(feature = "wasm-backend")]
use molt_backend::json_boundary::optional_u32;
use molt_backend::json_boundary::{
    expect_object, optional_bool, optional_string, required_field, required_string,
};
use serde_json::Value as JsonValue;

use super::model::{DaemonJobRequest, DaemonJobTarget};

impl DaemonJobRequest {
    /// Caller keys identify IR/codegen inputs; transport kind is independently
    /// owned here so even equal caller keys cannot replay a different artifact.
    pub(crate) fn artifact_cache_key(&self, key: &str) -> String {
        let key = key.trim();
        if key.is_empty() {
            return String::new();
        }
        match self.config_namespace.as_deref() {
            // Configurations share one daemon without evicting each other.
            Some(namespace) => format!(
                "artifact-v1:{namespace}:{}:{key}",
                self.target.artifact_kind()
            ),
            None => format!("artifact-v1:{}:{key}", self.target.artifact_kind()),
        }
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

impl DaemonJobTarget {
    /// The transport label every cache key of this job carries.
    fn artifact_kind(&self) -> &'static str {
        match self {
            #[cfg(feature = "native-backend")]
            Self::Native(options) => options.output_kind.as_str(),
            #[cfg(feature = "wasm-backend")]
            Self::Wasm(_) => "wasm",
        }
    }
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
mod tests {
    use super::*;

    #[cfg(feature = "native-backend")]
    #[test]
    fn native_artifact_job_contract_defaults_and_rejects_unknown_kinds() {
        use crate::backend_process::{NativeArtifactKind, NativeTargetOptions};
        let mut value =
            serde_json::json!({"id":"job", "is_wasm":false, "output":"output", "cache_key":"same"});
        let job = DaemonJobRequest::from_json_value(&value, "job").expect("default object job");
        assert_eq!(
            job.target,
            DaemonJobTarget::Native(NativeTargetOptions {
                target_triple: None,
                output_kind: NativeArtifactKind::Object,
            })
        );
        assert_eq!(job.artifact_cache_key("same"), "artifact-v1:object:same");
        assert!(job.artifact_cache_key("  ").is_empty());
        value["native_output_kind"] = "archive".into();
        let job = DaemonJobRequest::from_json_value(&value, "job").expect("archive job");
        assert_eq!(job.artifact_cache_key("same"), "artifact-v1:archive:same");
        value["is_wasm"] = true.into();
        assert!(DaemonJobRequest::from_json_value(&value, "job").is_err());
        value["is_wasm"] = false.into();
        for invalid in [
            serde_json::json!("unknown"),
            serde_json::json!(3),
            serde_json::json!(false),
        ] {
            value["native_output_kind"] = invalid;
            assert!(DaemonJobRequest::from_json_value(&value, "job").is_err());
        }
    }

    #[cfg(feature = "wasm-backend")]
    #[test]
    fn wasm_job_carries_its_layout_and_its_own_cache_label() {
        // src/molt/cli/backend_execution.py sends every layout field.
        let value = serde_json::json!({
            "id": "job", "is_wasm": true, "output": "app.wasm", "cache_key": "same",
            "target_triple": "wasm32-wasip1", "wasm_link": true, "wasm_data_base": 1048576,
            "wasm_table_base": 4096, "wasm_split_runtime_app_table_base": 8192,
        });
        let job = DaemonJobRequest::from_json_value(&value, "job").expect("wasm job");
        assert_eq!(
            job.target,
            DaemonJobTarget::Wasm(crate::backend_process::WasmTargetOptions {
                link_relocs: true,
                data_base: Some(1_048_576),
                table_base: Some(4096),
                split_runtime_app_table_base: Some(8192),
            })
        );
        assert_eq!(job.artifact_cache_key("same"), "artifact-v1:wasm:same");
    }

    #[cfg(not(feature = "wasm-backend"))]
    #[test]
    fn daemon_without_wasm_backend_refuses_wasm_jobs_when_parsing() {
        let value = serde_json::json!({
            "id": "job", "is_wasm": true, "output": "app.wasm", "cache_key": "same",
        });
        let error = DaemonJobRequest::from_json_value(&value, "request.jobs[0]")
            .expect_err("a native-only daemon cannot build WASM");
        assert!(error.contains("built without wasm-backend"), "{error}");
    }
}
