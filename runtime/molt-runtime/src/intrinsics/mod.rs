pub(crate) mod capabilities;
#[allow(dead_code)]
mod generated;
pub(crate) mod registry;

#[allow(unused_imports)]
pub(crate) use capabilities::*;
#[allow(unused_imports)]
pub(crate) use generated::{INTRINSICS, resolve_symbol};
pub(crate) use registry::install_into_builtins;
// Per-app intrinsic resolver entry point. On native this delegates to the
// app-emitted `molt_app_resolve_intrinsic` (registered before runtime init),
// keeping `resolve_symbol`/`resolve_core_symbol` native-unreachable so the
// linker dead-strips every unused intrinsic. On wasm it falls back to the
// staticlib `resolve_symbol` table.
//
// The cross-module consumer of this re-export is the wasm32-only reverse
// fn_ptr -> name trace in `call::function`; native callers live inside the
// `registry` module and reference the function directly, so the re-export is
// only needed (and only reachable) on wasm32.
#[cfg(target_arch = "wasm32")]
pub(crate) use registry::try_app_resolve_symbol;
