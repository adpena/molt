use molt_backend::backend_environment::{BackendEnv, BackendEnvironment};
use serde_json::Value as JsonValue;

/// Admits the request's backend settings. The daemon installs them on the
/// compiling thread for this request alone; it never writes them into the
/// process environment that concurrent requests share.
pub(super) fn daemon_request_environment(
    obj: &serde_json::Map<String, JsonValue>,
) -> Result<BackendEnvironment, String> {
    let environment = match obj.get("env") {
        None | Some(JsonValue::Null) => BackendEnvironment::default(),
        Some(JsonValue::Object(env)) => {
            BackendEnvironment::from_request(env).map_err(|err| format!("request.env: {err}"))?
        }
        Some(_) => return Err("request.env must be an object".to_string()),
    };
    if let Some(symbols) = environment.get(BackendEnv::MOLT_STDLIB_MODULE_SYMBOLS) {
        molt_backend::parse_stdlib_module_symbols(symbols)?;
    }
    Ok(environment)
}
