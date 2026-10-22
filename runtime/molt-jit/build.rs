//! Export the runner's `molt_isolate_bootstrap` to the runtime image.
//!
//! The runtime image leaves that symbol undefined; the dynamic loader binds it
//! to this executable when the runner loads the image. A Mach-O executable
//! exports its global symbols already. An ELF executable exports none unless
//! the link names them.

#![expect(
    clippy::disallowed_methods,
    reason = "Cargo hands a build script its target through the environment"
)]

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    let target_os = std::env::var("CARGO_CFG_TARGET_OS").unwrap_or_default();
    if matches!(target_os.as_str(), "linux" | "android" | "freebsd") {
        println!("cargo:rustc-link-arg-bins=-Wl,--export-dynamic-symbol=molt_isolate_bootstrap");
    }
}
