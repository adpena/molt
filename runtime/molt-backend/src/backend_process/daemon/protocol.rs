use molt_backend::json_boundary::{
    expect_object, optional_bool, optional_string, optional_u32, required_field, required_string,
};
use serde_json::Value as JsonValue;

use super::super::config::DAEMON_REQUEST_ENV_KEYS;

#[derive(Debug)]
#[cfg(any(unix, test))]
#[cfg_attr(
    not(any(feature = "native-backend", feature = "wasm-backend")),
    allow(dead_code)
)]
pub(crate) struct DaemonJobRequest {
    pub(crate) id: String,
    pub(crate) is_wasm: bool,
    #[cfg_attr(not(feature = "native-backend"), allow(dead_code))]
    pub(crate) target_triple: Option<String>,
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    pub(crate) wasm_link: bool,
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    pub(crate) wasm_data_base: Option<u32>,
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    pub(crate) wasm_table_base: Option<u32>,
    #[cfg_attr(not(feature = "wasm-backend"), allow(dead_code))]
    pub(crate) wasm_split_runtime_runtime_table_min: Option<u32>,
    pub(crate) output: String,
    pub(crate) cache_key: String,
    pub(crate) function_cache_key: Option<String>,
    pub(crate) skip_module_output_if_synced: bool,
    pub(crate) skip_function_output_if_synced: bool,
    pub(crate) probe_cache_only: bool,
    pub(crate) ir: Option<molt_backend::BackendIrDocument>,
    pub(crate) ir_path: Option<String>,
}

#[derive(Debug)]
#[cfg(any(unix, test))]
pub(crate) struct DaemonRequest {
    pub(crate) version: Option<u32>,
    pub(crate) ping: Option<bool>,
    pub(crate) include_health: Option<bool>,
    pub(crate) config_digest: Option<String>,
    pub(crate) jobs: Option<Vec<DaemonJobRequest>>,
}

#[derive(Debug)]
#[cfg(any(unix, test))]
pub(crate) struct DaemonJobResponse {
    pub(crate) id: String,
    pub(crate) ok: bool,
    pub(crate) cached: bool,
    pub(crate) cache_tier: Option<String>,
    pub(crate) output_written: bool,
    pub(crate) needs_ir: bool,
    pub(crate) message: Option<String>,
    /// Function names that were replaced with trap stubs due to Cranelift
    /// compilation failures.  Propagated to the CLI for build warnings.
    pub(crate) warnings: Vec<String>,
}

#[cfg(any(unix, test))]
pub(crate) fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug)]
#[cfg(any(unix, test))]
pub(crate) struct DaemonHealthResponse {
    pub(crate) protocol_version: u32,
    pub(crate) pid: u32,
    pub(crate) spawn_config_digest: Option<String>,
    pub(crate) active_config_digest: Option<String>,
    pub(crate) uptime_ms: u64,
    pub(crate) cache_entries: usize,
    pub(crate) cache_bytes: usize,
    pub(crate) cache_max_bytes: Option<usize>,
    pub(crate) request_limit_bytes: Option<usize>,
    pub(crate) max_jobs: Option<usize>,
    pub(crate) requests_total: u64,
    pub(crate) jobs_total: u64,
    pub(crate) cache_hits: u64,
    pub(crate) cache_misses: u64,
}

#[derive(Debug)]
#[cfg(any(unix, test))]
pub(crate) struct DaemonResponse {
    pub(crate) ok: bool,
    pub(crate) pong: bool,
    pub(crate) jobs: Vec<DaemonJobResponse>,
    pub(crate) error: Option<String>,
    pub(crate) health: Option<DaemonHealthResponse>,
}

#[cfg(any(unix, test))]
impl DaemonJobRequest {
    pub(crate) fn from_json_value(value: &JsonValue, ctx: &str) -> Result<Self, String> {
        let obj = expect_object(value, ctx)?;
        let is_wasm = required_field(obj, "is_wasm", ctx)?
            .as_bool()
            .ok_or_else(|| format!("{ctx}.is_wasm must be a bool"))?;
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
            is_wasm,
            target_triple: optional_string(obj, "target_triple", ctx)?,
            wasm_link: optional_bool(obj, "wasm_link", ctx)?.unwrap_or(false),
            wasm_data_base: optional_u32(obj, "wasm_data_base", ctx)?,
            wasm_table_base: optional_u32(obj, "wasm_table_base", ctx)?,
            wasm_split_runtime_runtime_table_min: optional_u32(
                obj,
                "wasm_split_runtime_runtime_table_min",
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

#[cfg(any(unix, test))]
impl DaemonRequest {
    pub(crate) fn from_json_bytes(bytes: &[u8]) -> Result<Self, String> {
        let value: JsonValue =
            serde_json::from_slice(bytes).map_err(|err| format!("invalid request JSON: {err}"))?;
        let obj = expect_object(&value, "request")?;
        let version = match obj.get("version") {
            None | Some(JsonValue::Null) => None,
            Some(value) => {
                let Some(raw) = value.as_u64() else {
                    return Err("request.version must be a non-negative integer".to_string());
                };
                Some(
                    u32::try_from(raw)
                        .map_err(|_| "request.version is out of range for u32".to_string())?,
                )
            }
        };
        let jobs = match obj.get("jobs") {
            None | Some(JsonValue::Null) => None,
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
                Some(out)
            }
        };
        // Apply per-request env var overrides so callers can control
        // backend diagnostics and non-TIR tuning without restarting the
        // daemon. TIR itself is not request-optional.
        for key in DAEMON_REQUEST_ENV_KEYS {
            unsafe {
                std::env::remove_var(key);
            }
        }
        if let Some(JsonValue::Object(env_map)) = obj.get("env") {
            for (key, val) in env_map {
                if let Some(s) = val.as_str() {
                    if key == molt_backend::STDLIB_MODULE_SYMBOLS_ENV {
                        molt_backend::parse_stdlib_module_symbols(s)?;
                    }
                    unsafe {
                        std::env::set_var(key, s);
                    }
                } else if key == molt_backend::STDLIB_MODULE_SYMBOLS_ENV {
                    return Err(format!(
                        "{} must be a string containing a JSON array of emitted module symbols",
                        molt_backend::STDLIB_MODULE_SYMBOLS_ENV
                    ));
                }
            }
        }
        Ok(Self {
            version,
            ping: optional_bool(obj, "ping", "request")?,
            include_health: optional_bool(obj, "include_health", "request")?,
            config_digest: optional_string(obj, "config_digest", "request")?,
            jobs,
        })
    }
}

#[cfg(any(unix, test))]
impl DaemonJobResponse {
    pub(crate) fn to_json_value(&self) -> JsonValue {
        let mut obj = serde_json::Map::new();
        obj.insert("id".to_string(), JsonValue::String(self.id.clone()));
        obj.insert("ok".to_string(), JsonValue::Bool(self.ok));
        obj.insert("cached".to_string(), JsonValue::Bool(self.cached));
        if let Some(cache_tier) = &self.cache_tier {
            obj.insert(
                "cache_tier".to_string(),
                JsonValue::String(cache_tier.clone()),
            );
        }
        obj.insert(
            "output_written".to_string(),
            JsonValue::Bool(self.output_written),
        );
        if !is_false(&self.needs_ir) {
            obj.insert("needs_ir".to_string(), JsonValue::Bool(self.needs_ir));
        }
        if let Some(message) = &self.message {
            obj.insert("message".to_string(), JsonValue::String(message.clone()));
        }
        if !self.warnings.is_empty() {
            obj.insert(
                "warnings".to_string(),
                JsonValue::Array(
                    self.warnings
                        .iter()
                        .map(|w| JsonValue::String(w.clone()))
                        .collect(),
                ),
            );
        }
        JsonValue::Object(obj)
    }
}

#[cfg(any(unix, test))]
impl DaemonHealthResponse {
    pub(crate) fn to_json_value(&self) -> JsonValue {
        let mut obj = serde_json::Map::new();
        obj.insert(
            "protocol_version".to_string(),
            JsonValue::from(self.protocol_version),
        );
        obj.insert("pid".to_string(), JsonValue::from(self.pid));
        if let Some(spawn_config_digest) = &self.spawn_config_digest {
            obj.insert(
                "spawn_config_digest".to_string(),
                JsonValue::String(spawn_config_digest.clone()),
            );
        }
        if let Some(active_config_digest) = &self.active_config_digest {
            obj.insert(
                "active_config_digest".to_string(),
                JsonValue::String(active_config_digest.clone()),
            );
        }
        obj.insert("uptime_ms".to_string(), JsonValue::from(self.uptime_ms));
        obj.insert(
            "cache_entries".to_string(),
            JsonValue::from(self.cache_entries),
        );
        obj.insert("cache_bytes".to_string(), JsonValue::from(self.cache_bytes));
        if let Some(cache_max_bytes) = self.cache_max_bytes {
            obj.insert(
                "cache_max_bytes".to_string(),
                JsonValue::from(cache_max_bytes),
            );
        }
        if let Some(request_limit_bytes) = self.request_limit_bytes {
            obj.insert(
                "request_limit_bytes".to_string(),
                JsonValue::from(request_limit_bytes),
            );
        }
        if let Some(max_jobs) = self.max_jobs {
            obj.insert("max_jobs".to_string(), JsonValue::from(max_jobs));
        }
        obj.insert(
            "requests_total".to_string(),
            JsonValue::from(self.requests_total),
        );
        obj.insert("jobs_total".to_string(), JsonValue::from(self.jobs_total));
        obj.insert("cache_hits".to_string(), JsonValue::from(self.cache_hits));
        obj.insert(
            "cache_misses".to_string(),
            JsonValue::from(self.cache_misses),
        );
        JsonValue::Object(obj)
    }
}

#[cfg(any(unix, test))]
impl DaemonResponse {
    pub(crate) fn to_json_value(&self) -> JsonValue {
        let mut obj = serde_json::Map::new();
        obj.insert("ok".to_string(), JsonValue::Bool(self.ok));
        obj.insert("pong".to_string(), JsonValue::Bool(self.pong));
        obj.insert(
            "jobs".to_string(),
            JsonValue::Array(
                self.jobs
                    .iter()
                    .map(DaemonJobResponse::to_json_value)
                    .collect(),
            ),
        );
        if let Some(error) = &self.error {
            obj.insert("error".to_string(), JsonValue::String(error.clone()));
        }
        if let Some(health) = &self.health {
            obj.insert("health".to_string(), health.to_json_value());
        }
        JsonValue::Object(obj)
    }
}
