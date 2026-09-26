//! Loader admission rejects missing runtime custody before opening a library.
//! Real extension execution belongs to runtime-backed acceptance lanes.

#![cfg(all(feature = "extension-loader", not(target_arch = "wasm32")))]

use molt_cpython_abi::loader::{LoadError, load_cpython_extension};

#[test]
fn loader_requires_runtime_before_opening_or_executing_extension() {
    // A missing file distinguishes the ordering: dlopen must never be reached.
    let error = unsafe {
        load_cpython_extension(std::path::Path::new("missing-extension"), "pkg.extension")
    }
    .expect_err("a hook-less bridge cannot initialize a runtime module");
    match error {
        LoadError::InitContractViolation { name, detail } => {
            assert_eq!(name, "pkg.extension");
            assert!(detail.contains("registered runtime extension-initialization hooks"));
        }
        other => panic!("runtime admission must precede library opening: {other:?}"),
    }
}
