//! End-to-end native CPython-ABI extension smoke test.
//!
//! The runtime, the CPython-ABI exports and the extension loader live in one
//! image: the `cext_host` example, built explicitly in this test's Cargo
//! target/profile. This executable compiles `hello.c` against that exact
//! image, loads it and runs the whole scenario inside it through one C entry
//! point, passing only C strings and a POD status. It links neither the
//! runtime nor the ABI itself. A missing host or C toolchain is a failure,
//! never a skip; loader admission alone is tested in molt-cpython-abi.

#![cfg(all(feature = "cext_loader", not(target_arch = "wasm32")))]

use std::ffi::{CStr, CString, c_char, c_int};
use std::path::Path;

mod cargo_test_artifacts {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_support/cargo_test_artifacts.rs"
    ));
}

mod cext_fixture {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../test_support/cext_fixture.rs"
    ));
}

type RunHello = unsafe extern "C" fn(*const c_char, *mut c_char, usize) -> c_int;

#[test]
fn hello_extension_runs_in_one_runtime_image() {
    let host = cext_fixture::HostArtifact::for_current_test_image("cext_host");
    let artifacts = cargo_test_artifacts::CargoTestArtifacts::new("cext-hello-host")
        .expect("create C-extension outputs within Cargo test artifact custody");
    let source = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../molt-cpython-abi/tests/c_extensions/hello.c");
    let extension = cext_fixture::build_extension(&artifacts, &host, &source, "hello");
    let extension = CString::new(
        extension
            .to_str()
            .expect("C-extension artifact path is UTF-8"),
    )
    .expect("C-extension artifact path has no NUL");

    let image = unsafe { libloading::Library::new(&host.library) }
        .unwrap_or_else(|error| panic!("load one-image host {:?}: {error}", host.library));
    let run: RunHello = unsafe {
        *image
            .get::<RunHello>(b"molt_cext_host_run_hello")
            .expect("host must export molt_cext_host_run_hello")
    };
    let mut diagnostic = vec![0 as c_char; 8192];
    let status = unsafe {
        run(
            extension.as_ptr(),
            diagnostic.as_mut_ptr(),
            diagnostic.len(),
        )
    };
    let diagnostic = unsafe { CStr::from_ptr(diagnostic.as_ptr()) }.to_string_lossy();
    // Runtime TLS destructors, the pinned extension and its callbacks belong
    // to this image; it stays mapped for the rest of the process.
    std::mem::forget(image);
    assert_eq!(status, 0, "one-image C-extension host failed: {diagnostic}");
    assert!(
        diagnostic.is_empty(),
        "successful host run left a diagnostic: {diagnostic}"
    );
}
