use molt_backend::json_boundary::{
    expect_object, optional_bool, optional_string, optional_u32, required_field, required_string,
};
use serde_json::Value as JsonValue;

use super::model::DaemonJobRequest;

impl DaemonJobRequest {
    /// Caller keys identify IR/codegen inputs; transport kind is independently
    /// owned here so even equal caller keys cannot replay a different artifact.
    pub(crate) fn artifact_cache_key(&self, key: &str) -> String {
        let key = key.trim();
        if key.is_empty() {
            return String::new();
        }
        let kind = if self.is_wasm {
            "wasm"
        } else {
            self.native_output_kind.as_str()
        };
        format!("artifact-v1:{kind}:{key}")
    }

    pub(crate) fn from_json_value(value: &JsonValue, ctx: &str) -> Result<Self, String> {
        let obj = expect_object(value, ctx)?;
        let is_wasm = required_field(obj, "is_wasm", ctx)?
            .as_bool()
            .ok_or_else(|| format!("{ctx}.is_wasm must be a bool"))?;
        let ir_path = optional_string(obj, "ir_path", ctx)?;
        let native_output_kind = optional_string(obj, "native_output_kind", ctx)?;
        if is_wasm && native_output_kind.is_some() {
            return Err(format!("{ctx}.native_output_kind requires a native target"));
        }
        let native_output_kind = native_output_kind
            .as_deref()
            .map(crate::backend_process::NativeArtifactKind::parse)
            .transpose()?
            .unwrap_or_default();
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
            is_wasm,
            target_triple: optional_string(obj, "target_triple", ctx)?,
            native_output_kind,
            wasm_link: optional_bool(obj, "wasm_link", ctx)?.unwrap_or(false),
            wasm_data_base: optional_u32(obj, "wasm_data_base", ctx)?,
            wasm_table_base: optional_u32(obj, "wasm_table_base", ctx)?,
            wasm_split_runtime_app_table_base: optional_u32(
                obj,
                "wasm_split_runtime_app_table_base",
                ctx,
            )?,
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
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend_process::NativeArtifactKind;

    #[test]
    fn native_artifact_job_contract_defaults_and_rejects_unknown_kinds() {
        let mut value =
            serde_json::json!({"id":"job", "is_wasm":false, "output":"output", "cache_key":"same"});
        let job = DaemonJobRequest::from_json_value(&value, "job").expect("default object job");
        assert_eq!(job.native_output_kind, NativeArtifactKind::Object);
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
}
