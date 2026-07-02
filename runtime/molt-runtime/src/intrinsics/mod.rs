pub(crate) mod capabilities;
#[allow(dead_code)]
mod generated;
pub(crate) mod registry;

#[allow(unused_imports)]
pub(crate) use capabilities::*;
#[allow(unused_imports)]
pub(crate) use generated::{INTRINSICS, resolve_symbol};
pub(crate) use registry::install_into_builtins;
// Per-app runtime-callable resolver entry point. Production WASM apps register
// a resolver emitted into the app object before runtime init. That resolver
// covers manifest-reachable intrinsic symbols and reachable builtin runtime
// callables, keeping monolithic generated resolvers native-unreachable so the
// linker dead-strips every unused callable. Unit tests keep `resolve_symbol`
// reachable because they validate the generated intrinsic registry directly.
//
// The cross-module consumer of this re-export is the wasm32-only reverse
// fn_ptr -> name trace in `call::function`; native callers live inside the
// `registry` module and reference the function directly, so the re-export is
// only needed (and only reachable) on wasm32.
#[cfg(target_arch = "wasm32")]
pub(crate) use registry::try_app_resolve_symbol;
