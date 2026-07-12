use serde_json::Value as JsonValue;

use super::super::super::super::config::DAEMON_REQUEST_ENV_KEYS;

pub(super) fn apply_daemon_request_env(
    obj: &serde_json::Map<String, JsonValue>,
) -> Result<(), String> {
    // Apply per-request env var overrides so callers can control backend
    // diagnostics and non-TIR tuning without restarting the daemon. TIR itself
    // is not request-optional.
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
    Ok(())
}
