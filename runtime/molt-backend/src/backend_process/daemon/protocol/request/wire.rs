use molt_backend::json_boundary::{expect_object, optional_bool, optional_string};
use serde_json::Value as JsonValue;

use super::env::apply_daemon_request_env;
use super::model::{DaemonJobRequest, DaemonRequest};

impl DaemonRequest {
    pub(crate) fn from_json_bytes(bytes: &[u8]) -> Result<Self, String> {
        let value: JsonValue =
            serde_json::from_slice(bytes).map_err(|err| format!("invalid request JSON: {err}"))?;
        let obj = expect_object(&value, "request")?;
        let version = daemon_request_version(obj)?;
        let jobs = daemon_request_jobs(obj)?;
        apply_daemon_request_env(obj)?;
        Ok(Self {
            version,
            ping: optional_bool(obj, "ping", "request")?,
            include_health: optional_bool(obj, "include_health", "request")?,
            config_digest: optional_string(obj, "config_digest", "request")?,
            jobs,
        })
    }
}

fn daemon_request_version(obj: &serde_json::Map<String, JsonValue>) -> Result<Option<u32>, String> {
    match obj.get("version") {
        None | Some(JsonValue::Null) => Ok(None),
        Some(value) => {
            let Some(raw) = value.as_u64() else {
                return Err("request.version must be a non-negative integer".to_string());
            };
            Ok(Some(u32::try_from(raw).map_err(|_| {
                "request.version is out of range for u32".to_string()
            })?))
        }
    }
}

fn daemon_request_jobs(
    obj: &serde_json::Map<String, JsonValue>,
) -> Result<Option<Vec<DaemonJobRequest>>, String> {
    match obj.get("jobs") {
        None | Some(JsonValue::Null) => Ok(None),
        Some(value) => {
            let array = value
                .as_array()
                .ok_or_else(|| "request.jobs must be an array".to_string())?;
            let mut out = Vec::with_capacity(array.len());
            for (idx, item) in array.iter().enumerate() {
                out.push(DaemonJobRequest::from_json_value(
                    item,
                    &format!("request.jobs[{idx}]"),
                )?);
            }
            Ok(Some(out))
        }
    }
}
