//! The deployment launch transport for host test manifests.
//!
//! The Rust mirror of `tests/wasm_execution_manifest.py::wasm_execution_launch()`
//! without grants: `molt.wasm_artifact.wasm_execution_manifest_launch` of the
//! deny policy (`resolve_runtime_policy_from_env({})`), which every deployment
//! carries when it grants no capability. Unit tests include this file by path
//! and integration tests through `support`, so every host test manifest
//! carries one launch shape. The host admits the digest as an opaque string and
//! the runtime checks only its form; this one is the deny policy's.

/// `launch.environment` of a deny-policy deployment.
pub fn deny_launch_environment() -> serde_json::Value {
    serde_json::json!({
        "MOLT_CAPABILITIES": "",
        "MOLT_CAPABILITY_TIER": "none",
        "MOLT_CAPABILITY_POLICY_DIGEST":
            "sha256:5d7b4da4ee77e8ef8e0e375c392807e69a4ca4d8c3b5553e4cd46801fee0d9de",
    })
}

/// `launch` of a deny-policy deployment.
pub fn deny_launch() -> serde_json::Value {
    serde_json::json!({"environment": deny_launch_environment()})
}
