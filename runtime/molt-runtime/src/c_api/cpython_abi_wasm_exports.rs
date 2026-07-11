//! WASM export anchors for the CPython ABI surface.
//!
//! Source-recompiled native extensions import raw CPython C-API symbols from
//! the split runtime. The implementation remains owned by `molt-cpython-abi`
//! and its C shim archive; this module only keeps the exact requested symbols
//! reachable so the WASM linker can publish them when the build asks for them.

#![allow(dead_code, improper_ctypes)]

mod variadic_exports {
    include!(concat!(
        env!("OUT_DIR"),
        "/molt_cpython_abi_variadic_exports.rs"
    ));
}

mod requested_exports {
    include!(concat!(
        env!("OUT_DIR"),
        "/molt_cpython_abi_requested_exports.rs"
    ));
}

#[used]
static MOLT_CPYTHON_ABI_WASM_EXPORT_ANCHOR_COUNT: extern "C" fn() -> usize =
    molt_cpython_abi_wasm_export_anchor_count;

#[unsafe(no_mangle)]
pub extern "C" fn molt_cpython_abi_wasm_export_anchor_count() -> usize {
    core::hint::black_box(variadic_exports::MOLT_CPYTHON_ABI_VARIADIC_EXPORT_ANCHORS.as_ptr());
    variadic_exports::MOLT_CPYTHON_ABI_VARIADIC_EXPORT_ANCHORS.len()
        + requested_exports::requested_export_anchor_count()
}
