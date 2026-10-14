//! Derive the backend's capability cfg aliases, so each predicate has one owner.

fn main() {
    println!("cargo:rerun-if-changed=build.rs");

    // The compile daemon serves native and WASM jobs over a Unix domain
    // socket. Emit `molt_backend_daemon` only for a Unix target with a backend
    // the daemon can run; other builds refuse `--daemon` when they parse it.
    println!("cargo:rustc-check-cfg=cfg(molt_backend_daemon)");
    let unix = std::env::var("CARGO_CFG_TARGET_FAMILY")
        .is_ok_and(|families| families.split(',').any(|family| family == "unix"));
    let daemon_backend = std::env::var_os("CARGO_FEATURE_NATIVE_BACKEND").is_some()
        || std::env::var_os("CARGO_FEATURE_WASM_BACKEND").is_some();
    if unix && daemon_backend {
        println!("cargo:rustc-cfg=molt_backend_daemon");
    }
}
