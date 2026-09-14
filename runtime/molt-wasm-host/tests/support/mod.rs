//! Shared production-host child-command custody for integration tests.

use std::process::Command;

/// Start an isolated host child without inheriting execution selectors.
///
/// Tests retain compile/performance/determinism knobs from the caller, then
/// deliberately add only the selector needed by that case.  This avoids an
/// ambient retired or precompiled-mode setting changing a production proof.
pub fn host() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_molt-wasm-host"));
    command
        .env_remove("MOLT_WASM_HOST_DEBUG")
        .env_remove("MOLT_WASM_HOST_LOG")
        .env_remove("MOLT_WASM_PRECOMPILED")
        .env_remove("MOLT_WASM_PRECOMPILED_WRITE")
        .env_remove("MOLT_WASM_PRECOMPILED_PATH")
        .env_remove("MOLT_WASM_PRECOMPILED_RUNTIME_PATH")
        .env_remove("MOLT_WASM_MANIFEST_PATH")
        .env_remove("MOLT_WASM_CACHE_CONFIG")
        .env("MOLT_WASM_CACHE", "0");
    command
}
