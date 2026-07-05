use serde_json::Value as JsonValue;

#[derive(Debug)]
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

pub(crate) fn is_false(value: &bool) -> bool {
    !*value
}

#[derive(Debug)]
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
pub(crate) struct DaemonResponse {
    pub(crate) ok: bool,
    pub(crate) pong: bool,
    pub(crate) jobs: Vec<DaemonJobResponse>,
    pub(crate) error: Option<String>,
    pub(crate) health: Option<DaemonHealthResponse>,
}

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
