use serde_json::Value as JsonValue;

use super::model::{DaemonHealthResponse, DaemonJobResponse, DaemonResponse, is_false};

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
